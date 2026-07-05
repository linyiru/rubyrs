//! Kernel-level builtins — `puts` / `p` / `pp` / `print` / `require`
//! plus the strict conversion functions (`Integer()` / `Float()` /
//! `String()`), introspection (`__method__` / `__callee__`), and
//! the `defined?` runtime helpers. Mirrors CRuby's `object.c` +
//! `io.c`'s output helpers.
//!
//! Dispatched via `Vm::builtin_call` from `do_call`'s no-receiver
//! branch.

use std::io::Write;

use crate::error::{RubyError, Trap};
use crate::heap::HeapObj;
use crate::value::Value;

use super::{PinGuard, Vm};

/// `(captured_self, lexical_class, named-local snapshot)` — the
/// context `extract_binding_ctx` recovers from a `Binding` instance.
type BindingCtx = (Value, Option<std::rc::Rc<crate::value::Class>>, Vec<(String, Value)>);
/// `(body, leading-param names, slot seed, registered source)` — the
/// compile inputs `prepare_eval_body` produces for `eval_string_full`.
type EvalBody = (crate::ast::SExpr, Vec<String>, Vec<(String, Value)>, String);

impl Vm {
    pub(crate) fn is_builtin_name(name: &str) -> bool {
        // Keep this list in sync with the match arms in `builtin_call`
        // below. Any name handled by `builtin_call` that is missing
        // here would let the toplevel fast path cache a user `def`
        // and silently shadow the builtin, which diverges from
        // master's "builtin always wins" dispatch order.
        //
        // There is also a *third* hand-maintained mirror of this set
        // inside the `"__defined_method?"` arm below (the
        // `let is_builtin = matches!(&*name, ...)` local). It is
        // pre-existing, already drifts from this list for several
        // names (`require_relative`, `__method__`, `__callee__`,
        // `block_given?`), and is out of scope for the PR that
        // introduced this gate — but maintainers updating
        // `builtin_call` should be aware that *three* places need to
        // change together, not two.
        matches!(
            name,
            "puts"
                | "p"
                | "pp"
                | "print"
                | "require"
                | "require_relative"
                | "Integer"
                | "Float"
                | "String"
                | "Array"
                | "Rational"
                | "sprintf"
                | "format"
                | "using"
                | "__time_now_raw"
                | "__rubyrs_time_parse_iso"
                | "sleep"
                | "exit"
                | "exit!"
                | "abort"
                | "warn"
                | "at_exit"
                | "__rubyrs_signal_trap"
                | "__rubyrs_stdout_write"
                | "__rubyrs_stderr_write"
                | "__rubyrs_exe_path"
                | "__method__"
                | "__callee__"
                | "block_given?"
                | "__defined_ivar?"
                | "__defined_method?"
                | "__defined_const?"
                | "__defined_recv_method?"
                | "__defined_super?"
                | "__defined_yield?"
                | "undef_method"
                | "eval"
                // `autoload(:Foo, "path")` / `autoload?(:Foo)`
                // top-level forms — Phase 1 of issue #224. The
                // class-recv forms (`Foo.autoload :Bar, ...`)
                // are still no-op stubs in dispatch.rs; Phase 2
                // wires those up to a per-Class registry.
                | "autoload"
                | "autoload?"
        )
    }

    /// Names that are valid as `Kernel.foo` / `Kernel::foo`
    /// explicit-receiver module-function calls AND are backed by a
    /// `builtin_call` arm. CRuby exposes Kernel's methods as
    /// `module_function`s: each is both a private instance method
    /// (the bare `foo` form, implicit self) and a public singleton
    /// method on the Kernel module object (the `Kernel.foo` form).
    /// rubyrs implements the bare form via `builtin_call`; the
    /// explicit-receiver dispatch in `do_call` routes a
    /// Kernel-module receiver back through `builtin_call` for any
    /// name in this set, so the two call shapes share one impl.
    ///
    /// Restricted to names `builtin_call` actually handles (so the
    /// route never silently falls through to a bogus
    /// `Some(Ok(nil))`). The rubyrs-internal helpers
    /// (`__time_now_raw`, `__rubyrs_signal_trap`, the `__defined_*?`
    /// reflection shims) are deliberately EXCLUDED — they aren't
    /// real CRuby Kernel methods and `Kernel.__time_now_raw` should
    /// stay a NoMethodError. Kernel module functions that rubyrs
    /// implements OUTSIDE `builtin_call` (`rand`, `raise`, `catch`,
    /// `throw`, ...) are also excluded here; they fall through to
    /// the normal not-found path as a known gap rather than
    /// mis-routing.
    pub(crate) fn is_kernel_module_function(name: &str) -> bool {
        matches!(
            name,
            "puts"
                | "p"
                | "pp"
                | "print"
                | "require"
                | "require_relative"
                | "load"
                | "eval"
                | "Integer"
                | "Float"
                | "String"
                | "Array"
                | "Rational"
                | "sprintf"
                | "format"
                | "sleep"
                | "exit"
                | "exit!"
                | "abort"
                | "warn"
                | "at_exit"
                | "caller"
                | "autoload"
                | "autoload?"
                | "block_given?"
                | "__method__"
                | "__callee__"
        )
    }

    /// `$stdout`-redirect probe. minitest's `capture_io` swaps
    /// `$stdout` for a StringIO; Kernel#puts/print/p must then route
    /// through the replacement object instead of the native sink.
    /// Returns the redirect target, or `None` when `$stdout` is
    /// unset or still an instance of our preamble IO veneer — whose
    /// own write path IS the native sink (this includes `.dup`ed
    /// copies, which carry the same `@which`, so a dup round-trip
    /// stays native). The same probe serves `$stderr` for `warn`.
    fn stdio_redirect(&mut self, global: &str, veneer_ok: bool) -> Option<Value> {
        let sym = self.interner.intern(global);
        let v = self.globals.get(&sym)?.clone();
        match &v {
            Value::Object(id) => {
                if veneer_ok && self.heap.class_of(*id).name == "IO" {
                    // A DELEGATING veneer ($stdout.reopen'd onto a
                    // Tempfile — capture_subprocess_io) must still
                    // route through dispatch so the veneer's write
                    // forwards to its target.
                    let delegate_sym = self.interner.intern("@delegate");
                    let delegating = self
                        .heap
                        .instance(*id)
                        .ivar_get(delegate_sym)
                        .is_some_and(|d| !matches!(d, Value::Nil));
                    if delegating { Some(v) } else { None }
                } else {
                    Some(v)
                }
            }
            _ => None,
        }
    }

    /// Forward a Kernel-level IO call (`puts`/`print`/`write`) to a
    /// redirected `$stdout`/`$stderr` object via full dispatch, so
    /// StringIO (or any user object with the right methods) sees it.
    fn forward_stdio_call(&mut self, target: Value, meth: &str, args: &[Value]) -> Result<Value, Trap> {
        let m_id = self.interner.intern(meth);
        self.stack.push(target);
        for a in args {
            self.stack.push(a.clone());
        }
        let pre = self.frames.len();
        self.do_call(m_id, args.len(), /*no_recv=*/false, u32::MAX)?;
        self.dispatch_until(pre)?;
        Ok(self.stack.pop().unwrap_or(Value::Nil))
    }

    /// TRUE when the current frame's `self` carries a USER method
    /// named `name` — a bare builtin call must defer to it (CRuby
    /// method lookup runs before Kernel's private builtins). rack
    /// overrides both `warn` (test-helper singleton capture) and
    /// `fail` (Files#fail returns a status triple instead of
    /// raising). Callers `return None` from their builtin arm on a
    /// hit so do_call falls through to normal dispatch.
    ///
    /// Deliberately NARROW: only consulted by specific builtin
    /// arms (warn / raise / fail), not as a general pre-gate —
    /// the broader builtin-shadowing question is the documented
    /// #491 own-table-early-gate design task.
    pub(crate) fn bare_builtin_user_override(&mut self, name: &str) -> bool {
        let id = self.interner.intern(name);
        let self_val = self.frames.last().map(|f| f.self_val.clone());
        match &self_val {
            Some(Value::Object(oid)) => {
                // `class_of` (not the bare real class) so a PER-INSTANCE
                // singleton override is seen too — rack's test helper
                // does `req.define_singleton_method(:warn)` to capture
                // deprecation warnings, and `Request#values_at` calls
                // bare `warn`; that must hit the singleton, not Kernel.
                let cls = self.heap.class_of(*oid);
                self.lookup_method_uncached(&cls, id).is_some()
            }
            Some(Value::Class(c)) => {
                self.lookup_class_singleton_method(c, id).is_some()
                    || self.lookup_class_object_instance_method(c, id).is_some()
            }
            _ => false,
        }
    }

    pub(crate) fn builtin_call(&mut self, name: &str, args: &[Value]) -> Option<Result<Value, Trap>> {
        match name {
            // --- Zlib host primitives (stdlib_vendor/zlib.rb veneer).
            // Bytes in via Value::Str, level/mtime via Value::Int.
            // Decompress failures surface as Zlib::DataError. ---
            #[cfg(feature = "stdlib")]
            "__zlib_deflate" | "__zlib_deflate_zlib" => {
                let (Some(Value::Str(s)), Some(Value::Int(lvl))) = (args.first(), args.get(1))
                else {
                    return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: "Zlib.deflate: expected (String, Integer)".into(),
                    })));
                };
                let data = s.content.borrow().to_vec();
                let out = if name == "__zlib_deflate" {
                    crate::zlib_native::deflate_raw(&data, *lvl)
                } else {
                    crate::zlib_native::deflate_zlib(&data, *lvl)
                };
                Some(Ok(Value::new_str_bytes_binary(out)))
            }
            #[cfg(feature = "stdlib")]
            "__zlib_inflate" | "__zlib_inflate_zlib" | "__zlib_inflate_auto" => {
                let Some(Value::Str(s)) = args.first() else {
                    return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: "Zlib.inflate: expected String".into(),
                    })));
                };
                let data = s.content.borrow().to_vec();
                let r = match name {
                    "__zlib_inflate" => crate::zlib_native::inflate_raw(&data),
                    "__zlib_inflate_zlib" => crate::zlib_native::inflate_zlib(&data),
                    _ => crate::zlib_native::inflate_auto(&data),
                };
                Some(match r {
                    Ok(out) => Ok(Value::new_str_bytes_binary(out)),
                    Err(e) => Err(self.trap(RubyError::HostException {
                        class_name: "Zlib::DataError".into(),
                        message: e,
                    })),
                })
            }
            #[cfg(feature = "stdlib")]
            "__zlib_gzip" => {
                let (Some(Value::Str(s)), Some(Value::Int(lvl)), Some(Value::Int(mtime))) =
                    (args.first(), args.get(1), args.get(2))
                else {
                    return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: "Zlib gzip: expected (String, Integer, Integer)".into(),
                    })));
                };
                let data = s.content.borrow().to_vec();
                let out = crate::zlib_native::gzip(&data, *lvl, *mtime as u32);
                Some(Ok(Value::new_str_bytes_binary(out)))
            }
            #[cfg(feature = "stdlib")]
            "__zlib_crc32" => {
                let Some(Value::Str(s)) = args.first() else {
                    return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: "Zlib crc32: expected String".into(),
                    })));
                };
                // `Zlib.crc32(string, prev_crc = 0)` — the optional second
                // arg continues a prior checksum.
                let init = match args.get(1) {
                    Some(Value::Int(n)) => *n as u32,
                    _ => 0,
                };
                let data = s.content.borrow().to_vec();
                Some(Ok(Value::Int(crate::zlib_native::crc32(&data, init) as i64)))
            }
            #[cfg(feature = "stdlib")]
            "__zlib_gunzip" => {
                let Some(Value::Str(s)) = args.first() else {
                    return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: "Zlib gunzip: expected String".into(),
                    })));
                };
                let data = s.content.borrow().to_vec();
                Some(match crate::zlib_native::gunzip(&data) {
                    Ok((out, mtime)) => {
                        self.maybe_gc();
                        if let Err(e) = self.check_alloc() {
                            return Some(Err(e));
                        }
                        let arr = vec![Value::new_str_bytes_binary(out), Value::Int(mtime as i64)];
                        Ok(Value::Array(self.heap.alloc(HeapObj::Array(arr.into()))))
                    }
                    Err(e) => Err(self.trap(RubyError::HostException {
                        class_name: "Zlib::GzipFile::Error".into(),
                        message: e,
                    })),
                })
            }
            // ----- stateful streaming gzip/inflate handles -----
            // Back Zlib::GzipWriter's incremental write/flush/finish and
            // Zlib::Inflate's stateful inflate (rack Deflater :sync path).
            #[cfg(feature = "stdlib")]
            "__zlib_gz_deflate_new" => {
                let (Some(Value::Int(lvl)), Some(Value::Int(mtime))) =
                    (args.first(), args.get(1))
                else {
                    return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: "gz_deflate_new: expected (Integer, Integer)".into(),
                    })));
                };
                let id = crate::zlib_native::gz_deflate_new(*lvl, *mtime as u32);
                Some(Ok(Value::Int(id as i64)))
            }
            #[cfg(feature = "stdlib")]
            "__zlib_gz_deflate_push" => {
                let (Some(Value::Int(id)), Some(Value::Str(s)), Some(Value::Int(flush))) =
                    (args.first(), args.get(1), args.get(2))
                else {
                    return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: "gz_deflate_push: expected (Integer, String, Integer)".into(),
                    })));
                };
                let data = s.content.borrow().to_vec();
                let out = crate::zlib_native::gz_deflate_push(*id as u64, &data, *flush);
                Some(Ok(Value::new_str_bytes_binary(out)))
            }
            #[cfg(feature = "stdlib")]
            "__zlib_inflate_stream_new" => {
                let Some(Value::Int(wbits)) = args.first() else {
                    return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: "inflate_stream_new: expected Integer".into(),
                    })));
                };
                let id = crate::zlib_native::inflate_stream_new(*wbits);
                Some(Ok(Value::Int(id as i64)))
            }
            #[cfg(feature = "stdlib")]
            "__zlib_inflate_stream_push" => {
                let (Some(Value::Int(id)), Some(Value::Str(s))) = (args.first(), args.get(1))
                else {
                    return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: "inflate_stream_push: expected (Integer, String)".into(),
                    })));
                };
                let data = s.content.borrow().to_vec();
                Some(match crate::zlib_native::inflate_stream_push(*id as u64, &data) {
                    Ok(out) => Ok(Value::new_str_bytes_binary(out)),
                    Err(e) => Err(self.trap(RubyError::HostException {
                        class_name: "Zlib::DataError".into(),
                        message: e,
                    })),
                })
            }
            #[cfg(feature = "stdlib")]
            "__zlib_stream_free" => {
                if let Some(Value::Int(id)) = args.first() {
                    crate::zlib_native::stream_free(*id as u64);
                }
                Some(Ok(Value::Nil))
            }
            // `URI.decode_www_form_component` hot path: `%XX` → byte,
            // `+` → space, else verbatim. The pure-Ruby fallback
            // materialized the whole input as an Array of byte Integers
            // and looped in Ruby (~256ns/byte — a 128 MB POST body took
            // tens of seconds); this is a Rust byte scan (~CRuby speed).
            // Returns the decoded bytes (BINARY) on success, or `nil`
            // when a `%` is not followed by two hex digits — the caller
            // raises ArgumentError with the original string in the msg.
            // Reachable only through the stdlib `uri` veneer.
            #[cfg(feature = "stdlib")]
            "__uri_decode_www_form" => {
                let Some(Value::Str(s)) = args.first() else {
                    return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: "__uri_decode_www_form: expected String".into(),
                    })));
                };
                #[inline]
                fn hex_val(b: u8) -> Option<u8> {
                    match b {
                        b'0'..=b'9' => Some(b - b'0'),
                        b'a'..=b'f' => Some(b - b'a' + 10),
                        b'A'..=b'F' => Some(b - b'A' + 10),
                        _ => None,
                    }
                }
                let bytes = s.content.borrow();
                let n = bytes.len();
                let mut out: Vec<u8> = Vec::with_capacity(n);
                let mut i = 0usize;
                let mut invalid = false;
                while i < n {
                    let b = bytes[i];
                    if b == b'%' {
                        match (
                            bytes.get(i + 1).copied().and_then(hex_val),
                            bytes.get(i + 2).copied().and_then(hex_val),
                        ) {
                            (Some(h1), Some(h2)) => {
                                out.push(h1 * 16 + h2);
                                i += 3;
                            }
                            _ => {
                                invalid = true;
                                break;
                            }
                        }
                    } else if b == b'+' {
                        out.push(b' ');
                        i += 1;
                    } else {
                        out.push(b);
                        i += 1;
                    }
                }
                Some(Ok(if invalid {
                    Value::Nil
                } else {
                    Value::new_str_bytes_binary(out)
                }))
            }
            // `Kernel#binding` — capture the CALLER's scope (self +
            // lexical class) into a Binding instance so `eval(src,
            // binding)` runs with that self. The self-dispatch layer
            // (rack's Builder.new_from_string evals a rackup script —
            // `run`/`use`/`map` — against `builder.instance_eval {
            // binding }`). Outer local-variable capture is a follow-up.
            "binding" if args.is_empty() => {
                let self_val = self
                    .frames
                    .last()
                    .map(|f| f.self_val.clone())
                    .unwrap_or(Value::Nil);
                let lex = self.class_stack.last().cloned();
                let Some(bcls) = self.classes.get(&self.interner.intern("Binding")).cloned()
                else {
                    return Some(Ok(Value::Nil));
                };
                // Snapshot the capturing frame's NAMED locals (slot →
                // value) so `eval(src, binding)` can re-seed them. The
                // `binding` builtin runs inline in the caller's frame,
                // so `frames.last()` IS the method whose locals we want.
                let mut snap: Vec<(String, Value)> = Vec::new();
                if let Some(frame) = self.frames.last() {
                    let proto_idx = frame.proto_idx;
                    // Capture the frame's locals storage ONCE (clone the
                    // shared cell / note the stack base) so the per-slot
                    // read below doesn't re-borrow `self.frames` — a
                    // re-borrow would force an unwrap of `frames.last()`,
                    // which the panic-budget ratchet counts.
                    let shared = frame.locals.as_shared().cloned();
                    let stack_base = match &frame.locals {
                        crate::vm::Locals::Stack(b) => Some(*b as usize),
                        crate::vm::Locals::Shared(_) => None,
                    };
                    // Capture routing for a block frame's outer slots
                    // (mirror of Op::LoadLocal).
                    let n = self.protos[proto_idx].n_locals as usize;
                    for slot in 0..n {
                        let name = self.protos[proto_idx]
                            .local_names
                            .get(slot)
                            .cloned()
                            .unwrap_or_default();
                        if name.is_empty() {
                            continue;
                        }
                        let val = if let Some(cell) = frame.outer_cell_for(slot) {
                            cell.borrow().get(slot).cloned().unwrap_or(Value::Nil)
                        } else if let Some(rc) = &shared {
                            rc.borrow().get(slot).cloned().unwrap_or(Value::Nil)
                        } else if let Some(base) = stack_base {
                            self.locals_arena
                                .get(base + slot)
                                .cloned()
                                .unwrap_or(Value::Nil)
                        } else {
                            Value::Nil
                        };
                        snap.push((name, val));
                    }
                }
                self.maybe_gc();
                if let Err(e) = self.check_alloc() {
                    return Some(Err(e));
                }
                let mut ivars = crate::value::IvarTable::default();
                ivars.insert(&bcls, self.interner.intern("@__self"), self_val);
                if let Some(c) = lex {
                    ivars.insert(&bcls, self.interner.intern("@__lexical_class"), Value::Class(c));
                }
                let id = self.heap.alloc(HeapObj::Instance(crate::value::Instance {
                    class: bcls,
                    ivars,
                    singleton_class: None,
                    frozen: std::cell::Cell::new(false),
                }));
                if !snap.is_empty() {
                    self.binding_locals.insert(id.0 as usize, snap);
                }
                Some(Ok(Value::Object(id)))
            }
            "puts" => {
                if let Some(target) = self.stdio_redirect("$stdout", true) {
                    return Some(self.forward_stdio_call(target, "puts", args));
                }
                // CRuby's `puts` flattens arrays: each element is
                // printed on its own line, recursively. Empty
                // string still gets a newline (so `puts ""` and
                // `puts` look identical). Empty array prints
                // nothing.
                fn puts_one(vm: &mut Vm, v: &Value) -> Result<(), Trap> {
                    match v {
                        Value::Array(id) => {
                            let snapshot: Vec<Value> = vm.heap.array(*id).clone();
                            // Pin the snapshot across the recursive
                            // to_s dispatch (user code → GC).
                            let mut g = PinGuard::new(vm);
                            g.pin(Value::Array(*id));
                            for item in &snapshot { g.pin(item.clone()); }
                            for item in &snapshot { puts_one(g.vm, item)?; }
                        }
                        _ => {
                            // Dispatch a user `to_s` override; native otherwise.
                            let s = vm.stringify_for_output(v, false)?;
                            // CRuby: `puts` skips the trailing
                            // newline if the value already ends in
                            // one. Avoids the double-blank-line
                            // surprise on `puts result` where
                            // `result` is a `"...\n"` builder
                            // string.
                            if s.ends_with('\n') {
                                let _ = write!(vm.stdout, "{}", s);
                            } else {
                                let _ = writeln!(vm.stdout, "{}", s);
                            }
                        }
                    }
                    Ok(())
                }
                if args.is_empty() {
                    let _ = writeln!(self.stdout);
                } else {
                    let pinned: Vec<Value> = args.to_vec();
                    let mut g = PinGuard::new(self);
                    for a in &pinned { g.pin(a.clone()); }
                    for a in &pinned {
                        if let Err(t) = puts_one(g.vm, a) {
                            return Some(Err(t));
                        }
                    }
                }
                Some(Ok(Value::Nil))
            }
            // Kernel#p — print each arg's `inspect` form, one per
            // line. Return value mirrors CRuby: nil for zero args,
            // the lone arg for one, an Array of the args for more.
            // `pp` is aliased to `p` (we don't have a pretty-
            // printer; single-line inspect is sufficient for our
            // subset and matches CRuby for simple values).
            // `__method__` / `__callee__` — return the enclosing
            // method's name as a Symbol, or nil at the toplevel.
            // Walks the frame stack from the top, skipping block
            // and class-body frames so a `def foo; arr.each { ... }`
            // inside reports `:foo` from inside the block. CRuby
            // distinguishes the two when method aliasing is
            // involved — we don't model aliases, so both resolve
            // to the same name.
            // `Kernel#caller` — backtrace as Array<String>. CRuby
            // shape: `caller(start=1, length=nil)`.
            //   - `caller` ≡ `caller(1)` — both skip the frame
            //     containing the `caller` call (the calling
            //     method's own frame).
            //   - `caller(0)` — INCLUDES the calling method's
            //     frame at the head of the array (start is
            //     absolute, not "additional skip").
            //   - `caller(n)` — `n` is the absolute start
            //     index; frames before index `n` are dropped.
            //     `n` here is NOT relative to the default;
            //     `caller(2)` skips two frames (one more than
            //     `caller(1)`).
            //   - `caller(n, l)` — at most `l` entries
            //     starting at index `n`.
            //   - `start > depth` → returns `nil` (not an
            //     empty array).
            // Output order is most-recent first (immediate
            // caller at index 0, older frames later).
            // Each entry: "filename:line:in 'method'" — single
            // quotes (CRuby 3.x). Sinatra-4's `cleaned_caller`
            // (sinatra/base.rb:1913) parses this format with
            // `split(/:(?=\d|in )/, 3)`, so the colons and
            // `in '...'` literal must match.
            //
            // Tier-1 scope: positional Integer args only. CRuby
            // also accepts `caller(range)`; that lands as a
            // follow-up. (TRY_RUNS pass-12 layer #15.)
            //
            // INVARIANT — DELIBERATELY NOT IN `is_builtin_name`:
            // `caller` lives in `builtin_call` (so it dispatches
            // as a Kernel builtin when no shadow is present) but
            // is INTENTIONALLY OMITTED from `Vm::is_builtin_name`
            // at the top of this file. That gate disables the
            // toplevel-method fast path for builtin names —
            // including it would prevent user code
            // (`def caller; end`) from shadowing the builtin,
            // which CRuby DOES allow (verified via `ruby -e`)
            // and which the `tests/fixtures/errors/nomethod.rb`
            // integration test depends on. If you're sweeping
            // the three sync-required lists into shape, leave
            // `caller` out of `is_builtin_name`. Code-review
            // #342 round 6.
            //
            // Code-review #342 round 3 corrected the above
            // explanation — earlier wording said "skip an
            // additional n frames" / "Ruby caller of the
            // `caller` call site", both of which read backwards.
            "caller" => {
                // CRuby distinguishes arity errors from coercion
                // errors here: wrong NUMBER of args → ArgumentError,
                // wrong TYPE → TypeError("no implicit conversion of
                // <X> into Integer"). Mirror that split so code that
                // catches one but not the other behaves the same.
                // (Code-review #342 round 1.)
                if args.len() > 2 {
                    return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: format!(
                            "wrong number of arguments (given {}, expected 0..2)",
                            args.len(),
                        ),
                    })));
                }
                // `caller(1..1)` Range form (probed on CRuby 3.4.1):
                // inclusive end; endless → unbounded; beginless →
                // begin 0; empty span (3..1) → []; begin past the
                // stack depth → nil (handled by the shared skip >
                // total check below via the rewritten args).
                // minitest mock.rb's KW_WARNED path does
                // `caller(1..1).first`.
                let range_args: Option<(i64, i64)> = match args {
                    [Value::Range(rid)] => {
                        let r = self.heap.range(*rid);
                        let b = match &r.begin {
                            Value::Int(n) => *n,
                            Value::Nil => 0,
                            other => {
                                return Some(Err(self.trap(RubyError::TypeError {
                                    msg: format!(
                                        "no implicit conversion of {} into Integer",
                                        other.conv_type_name(),
                                    ),
                                })));
                            }
                        };
                        let e = match &r.end {
                            Value::Int(n) => Some(if r.exclusive { *n - 1 } else { *n }),
                            Value::Nil => None,
                            other => {
                                return Some(Err(self.trap(RubyError::TypeError {
                                    msg: format!(
                                        "no implicit conversion of {} into Integer",
                                        other.conv_type_name(),
                                    ),
                                })));
                            }
                        };
                        if b < 0 || e.is_some_and(|e| e < 0) {
                            return Some(Err(self.trap(RubyError::ArgumentError {
                                msg: "negative level".to_string(),
                            })));
                        }
                        match e {
                            // Empty span → [] (NOT nil), unless the
                            // begin itself is past the depth.
                            Some(e) if e < b => {
                                if (b as usize) > self.frames.len() {
                                    return Some(Ok(Value::Nil));
                                }
                                self.maybe_gc();
                                if let Err(t) = self.check_alloc() { return Some(Err(t)); }
                                let id = self.heap.alloc(HeapObj::Array(Vec::new().into()));
                                return Some(Ok(Value::Array(id)));
                            }
                            Some(e) => Some((b, e - b + 1)),
                            None => Some((b, i64::MAX)),
                        }
                    }
                    _ => None,
                };
                let range_buf;
                let args: &[Value] = if let Some((b, l)) = range_args {
                    range_buf = [Value::Int(b), Value::Int(l)];
                    &range_buf
                } else {
                    args
                };
                for a in args.iter() {
                    // `Value::BigInt` IS an Integer in rubyrs's
                    // bignum build (just one that doesn't fit
                    // in i64). Accept it here — converting to
                    // an in-range value (or raising RangeError
                    // on overflow) happens below. Without this,
                    // `caller(2**100)` would produce the absurd
                    // "no implicit conversion of Integer into
                    // Integer" message. Code-review #342
                    // round 7.
                    //
                    // The BigInt variant is gated behind the
                    // `bignum` feature (value.rs:116); in the
                    // non-bignum build there's no BigInt to
                    // match against, so the pattern stays
                    // `Value::Int(_)` only. Code-review #342
                    // round 8.
                    #[cfg(feature = "bignum")]
                    let is_int = matches!(a, Value::Int(_) | Value::BigInt(_));
                    #[cfg(not(feature = "bignum"))]
                    let is_int = matches!(a, Value::Int(_));
                    if !is_int {
                        // CRuby uses a distinct phrasing for nil
                        // ("from nil to integer") versus other
                        // types ("of <Class> into Integer") —
                        // `Value::num2int_conv_msg` (the shared
                        // rb_num2long-shaped message helper; was a
                        // hand-rolled map with identical output).
                        // Code-review #342 round 2.
                        return Some(Err(self.trap(RubyError::TypeError {
                            msg: a.num2int_conv_msg(),
                        })));
                    }
                }
                // BigInt → i64 coercion: CRuby raises RangeError
                // ("bignum too big to convert into 'long'") when
                // the value doesn't fit, NOT TypeError or "negative
                // level". Convert each arg up front; on overflow,
                // raise RangeError with CRuby's wording. Done in a
                // separate sweep from the type-check so the result
                // can be reused by the match below without
                // re-borrowing `self.heap` inside the pattern arms.
                //
                // The BigInt arm is gated behind the `bignum`
                // feature; `ToPrimitive` is only needed when the
                // arm exists, so its import is gated too —
                // otherwise the non-bignum build trips
                // `unused_imports`. Code-review #342 round 8.
                #[cfg(feature = "bignum")]
                use num_traits::ToPrimitive;
                let mut converted: Vec<i64> = Vec::with_capacity(args.len());
                for a in args.iter() {
                    let n = match a {
                        Value::Int(n) => *n,
                        #[cfg(feature = "bignum")]
                        Value::BigInt(id) => {
                            match self.heap.bigint(*id).to_i64() {
                                Some(n) => n,
                                None => {
                                    return Some(Err(self.trap(RubyError::RangeError {
                                        msg: "bignum too big to convert into 'long'".to_string(),
                                    })));
                                }
                            }
                        }
                        _ => unreachable!("type-checked above"),
                    };
                    converted.push(n);
                }
                // Saturating i64 → usize: on 32-bit targets (the
                // repo builds `wasm32-wasip1` in CI) `as usize`
                // would truncate large positives, so a huge
                // `start` value could wrap back into range and
                // produce frames instead of `nil`. Clamp to
                // `usize::MAX` instead — a usize::MAX skip will
                // never produce frames (since total is bounded
                // by the actual frame stack depth), preserving
                // the "beyond depth → nil" behavior on every
                // target. Code-review #342 round 5.
                let to_usize_sat = |n: i64| -> usize {
                    usize::try_from(n).unwrap_or(usize::MAX)
                };
                let (skip, limit) = match converted.as_slice() {
                    [] => (1usize, usize::MAX),
                    [n] if *n >= 0 => (to_usize_sat(*n), usize::MAX),
                    [n, l] if *n >= 0 && *l >= 0 => {
                        (to_usize_sat(*n), to_usize_sat(*l))
                    }
                    // At least one arg is negative.
                    _ => {
                        return Some(Err(self.trap(RubyError::ArgumentError {
                            msg: "negative level".to_string(),
                        })));
                    }
                };
                // Walk from top of stack downward; skip `skip`
                // frames first, then collect up to `limit`.
                let total = self.frames.len();
                if skip > total {
                    // start beyond depth → CRuby returns nil.
                    return Some(Ok(Value::Nil));
                }
                // Capacity hint: at most `total - skip` frames
                // (collected when limit is unbounded), capped by
                // `limit` when it's smaller. Avoids the usual
                // 0/4/8/... reallocation walk under deep stacks
                // — this runs in hot-path framework code like
                // Sinatra's `cleaned_caller`. Code-review #342
                // round 5.
                let cap = (total - skip).min(limit);
                let mut out: Vec<Value> = Vec::with_capacity(cap);
                for (i, f) in self.frames.iter().rev().enumerate() {
                    if i < skip { continue; }
                    if out.len() >= limit { break; }
                    let proto = &self.protos[f.proto_idx];
                    let op_ip = if f.ip == 0 { 0 } else { f.ip - 1 };
                    let span = proto
                        .op_spans
                        .get(op_ip)
                        .copied()
                        .unwrap_or(crate::error::Span::ZERO);
                    let line = match self.sources.get(proto.filename.as_ref()) {
                        Some(src) => crate::error::line_col(src, span.byte_offset).0,
                        None => 0,
                    };
                    let s = format!("{}:{}:in '{}'", proto.filename, line, proto.name);
                    out.push(Value::new_str(s));
                }
                // GC discipline mirrors the other Kernel arms that
                // hand back a fresh heap Array (e.g. `methods` /
                // `local_variables`): give the heap a chance to
                // sweep before allocating, then refuse if we'd
                // blow `Config::max_heap_objects`. `out` holds
                // only `Value::Str` (`Rc<RStr>` — not on the GC
                // heap), so no pinning is required across
                // `maybe_gc`. (Code-review #342 round 1.)
                self.maybe_gc();
                if let Err(t) = self.check_alloc() {
                    return Some(Err(t));
                }
                let id = self.heap.alloc(crate::heap::HeapObj::Array(out.into()));
                Some(Ok(Value::Array(id)))
            }
            "__method__" | "__callee__" => {
                let name_opt: Option<String> = {
                    let mut found = None;
                    for f in self.frames.iter().rev() {
                        if f.is_block || f.is_class_body { continue; }
                        // define_method frames stamp their RUNTIME
                        // name in aux (the proto name is the block's
                        // lexical context, e.g. "<block>").
                        if let Some(nm) = f.aux.as_ref().and_then(|a| a.invoked_name) {
                            found = Some(self.interner.resolve(nm).to_string());
                            break;
                        }
                        let n = &self.protos[f.proto_idx].name;
                        if n == "<main>" { break; }
                        found = Some(n.clone());
                        break;
                    }
                    found
                };
                let result = match name_opt {
                    Some(n) => Value::Sym(self.interner.intern(&n)),
                    None => Value::Nil,
                };
                Some(Ok(result))
            }
            "block_given?" => {
                // CRuby semantics: walks past block frames to the
                // enclosing method frame, then reports whether that
                // method was called with a block. Inside an iterator
                // block (`each { block_given? }`), reads the
                // surrounding method's block-arg, not the block's
                // own slot. Toplevel `<main>` answers false (no
                // method context, no block to inherit).
                //
                // Arity 0 — CRuby raises ArgumentError on any args
                // (verified against `ruby -e`). Silently ignoring
                // extras would hide caller bugs.
                if !args.is_empty() {
                    return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: format!("wrong number of arguments (given {}, expected 0)", args.len()),
                    })));
                }
                // Resolve the SAME block `yield`/`super` resolve: in a
                // block frame the captured `captured_yield_block`, in a
                // method frame its own `block_arg` (mirrors lookup.rs's
                // super dispatch). This must NOT re-walk to a lexical-owner
                // frame on the stack: a block stored as a Proc and `.call`ed
                // AFTER its enclosing method returned (RuboCop's `opts.on {
                // |a| ...; yield a if block_given? }`, run later by
                // `parse!`) has no owner frame left to walk to, but its
                // `captured_yield_block` still carries the method's block —
                // exactly what CRuby's closure reports. The old
                // `lexical_owner_of_top` walk returned false there (so the
                // deferred `yield` never fired), even though a bare `yield`
                // in the same block worked. It also still reports the
                // enclosing method's block for `def helper; yield; end; def
                // m; helper { block_given? }` because the block's
                // `captured_yield_block` is m's block, captured at creation.
                let has_block = self
                    .frames
                    .last()
                    .map(|f| if f.is_block { f.captured_yield_block.is_some() } else { f.block_arg.is_some() })
                    .unwrap_or(false);
                Some(Ok(Value::Bool(has_block)))
            }
            // `defined?` plumbing: three runtime checks that
            // resolve against `self` (ivars), the class chain
            // (methods), and the constant table. AST translation
            // routes here for IVarRead / Call / ConstRead inner
            // expressions. The label-only-on-hit pattern matches
            // CRuby: hit returns a String, miss returns nil.
            "__defined_yield?" => {
                // `defined?(yield)` — "yield" iff the enclosing method
                // was called with a block, else nil (same resolution as
                // `block_given?`: the block frame's captured block, or a
                // method frame's own block_arg — survives a deferred Proc
                // whose enclosing method already returned). sequel's
                // Database.connect gates `return yield(db)` on `if
                // defined?(yield)`; with the old catch-all "expression"
                // label it always ran the yield and raised "no block given".
                let has_block = self
                    .frames
                    .last()
                    .map(|f| if f.is_block { f.captured_yield_block.is_some() } else { f.block_arg.is_some() })
                    .unwrap_or(false);
                Some(Ok(if has_block { Value::new_str("yield") } else { Value::Nil }))
            }
            "__defined_super?" => {
                // `defined?(super)` — "super" iff the enclosing method
                // has a same-named method further up the chain. Host fns
                // run inline (no frame push), so the top frame is still
                // the method that textually contains the `defined?`. Get
                // its method name from the proto and probe via
                // `super_lookup` (which resolves WITHOUT invoking);
                // Ok → a super exists, Err → none. Synthetic block / eval
                // protos (name starts with `<`) aren't modeled — return
                // nil there (a `defined?(super)` inside a block is rare).
                let name_id = self.frames.last().and_then(|f| {
                    let pn = self.protos[f.proto_idx].name.clone();
                    if pn.is_empty() || pn.starts_with('<') {
                        None
                    } else {
                        Some(self.interner.intern(&pn))
                    }
                });
                let has = match name_id {
                    Some(nid) => self.super_lookup(nid).is_ok(),
                    None => false,
                };
                Some(Ok(if has { Value::new_str("super") } else { Value::Nil }))
            }
            "__defined_ivar?" => {
                if let Some(Value::Sym(sid)) = args.first() {
                    let self_val = self.frames.last()
                        .map(|f| f.self_val.clone())
                        .unwrap_or(Value::Nil);
                    let hit = match &self_val {
                        Value::Object(oid) => {
                            self.heap.instance(*oid).ivar_defined(*sid)
                        }
                        // Class-level ivars: `defined?(@name)` inside
                        // `def self.x` reads the class object's own
                        // table — minitest Spec::DSL's
                        // `defined?(@name) ? @name : super` name
                        // resolution on describe-created classes.
                        Value::Class(cls) => cls.ivars.borrow().contains_key(sid),
                        _ => false,
                    };
                    return Some(Ok(if hit { Value::new_str("instance-variable") } else { Value::Nil }));
                }
                Some(Ok(Value::Nil))
            }
            "__defined_method?" => {
                if let Some(Value::Sym(sid)) = args.first() {
                    // Resolution order mirrors `do_call`'s no-recv
                    // path: builtin → host fn → self.class methods
                    // → toplevel.
                    let name = self.interner.resolve(*sid).clone();
                    let is_builtin = matches!(
                        &*name,
                        "puts" | "p" | "pp" | "print" | "require" | "load" |
                        "sprintf" | "format" | "__time_now_raw" | "__rubyrs_time_parse_iso" | "sleep" |
                        "exit" | "exit!" | "abort" | "warn" | "at_exit" | "__rubyrs_signal_trap" |
                        "__rubyrs_stdout_write" | "__rubyrs_stderr_write" | "__rubyrs_exe_path" |
                        "Integer" | "Float" | "String" | "Array" | "Rational" |
                        "eval" | "caller" |
                        "__defined_ivar?" | "__defined_method?" | "__defined_const?" |
                        "autoload" | "autoload?"
                    );
                    let host_hit = self.host_fns.contains_key(sid);
                    let self_val = self.frames.last()
                        .map(|f| f.self_val.clone())
                        .unwrap_or(Value::Nil);
                    let class_hit = match &self_val {
                        Value::Object(oid) => {
                            let cls = self.heap.instance(*oid).class.clone();
                            self.lookup_method_uncached(&cls, *sid).is_some()
                        }
                        // Inside a class/module BODY (self is the
                        // class object) a bare name resolves
                        // through the class-object instance chain
                        // (Class/Module reopens): minitest's mock.rb
                        // guards `infect_an_assertion ... if
                        // defined?(infect_an_assertion)` inside
                        // `module Minitest::Expectations`, and
                        // infect_an_assertion lives on Module.
                        Value::Class(c) => {
                            self.lookup_class_singleton_method(c, *sid).is_some()
                                || self.lookup_class_object_instance_method(c, *sid).is_some()
                        }
                        _ => false,
                    };
                    let toplevel_hit = self.toplevel_methods.contains_key(sid);
                    let hit = is_builtin || host_hit || class_hit || toplevel_hit;
                    return Some(Ok(if hit { Value::new_str("method") } else { Value::Nil }));
                }
                Some(Ok(Value::Nil))
            }
            // `defined?(recv.m)` with an already-evaluated receiver
            // (ast.rs lowers the side-effect-free receiver shapes
            // to this; const receivers arrive guarded by a
            // `__defined_const?` check so the receiver eval can't
            // NameError). Pure method-table check via `responds_to`
            // — deliberately NOT consulting respond_to_missing?,
            // matching CRuby's defined? (method-entry lookup, not
            // the respond_to? protocol).
            "__defined_recv_method?" => {
                if let (Some(recv), Some(Value::Sym(sid))) = (args.first(), args.get(1)) {
                    let hit = self.responds_to(recv, *sid, false);
                    return Some(Ok(if hit { Value::new_str("method") } else { Value::Nil }));
                }
                Some(Ok(Value::Nil))
            }
            // `undef :name` inside instance_eval — ast.rs lowers
            // undef to a bare `undef_method` call, which arrives
            // here with an OBJECT self (class-body forms have a
            // Class self and fall through — `return None` — to the
            // class intrinsics arm in dispatch.rs). Tombstone the
            // name(s) on the object's eigenclass so lookup stops
            // there: rack's rewindable_input spec builds a
            // non-rewindable IO via
            // `io.instance_eval { undef :rewind }`.
            "undef_method" => {
                if self.bare_builtin_user_override("undef_method") {
                    return None;
                }
                let self_val = self.frames.last().map(|f| f.self_val.clone());
                let Some(Value::Object(oid)) = self_val else {
                    return None;
                };
                let mut sids = Vec::with_capacity(args.len());
                for arg in args {
                    let sid = match arg {
                        Value::Sym(s) => *s,
                        Value::Str(s) => self.interner.intern(&s.to_string_lossy()),
                        other => {
                            let inspected = other.to_inspect(&self.heap, &self.interner);
                            return Some(Err(self.trap(RubyError::TypeError {
                                msg: format!("{} is not a symbol nor a string", inspected),
                            })));
                        }
                    };
                    sids.push(sid);
                }
                let sc = self.heap.ensure_singleton_class(oid);
                for sid in sids {
                    sc.undefed.borrow_mut().insert(sid);
                }
                self.method_gen = self.method_gen.wrapping_add(1);
                Some(Ok(Value::Nil))
            }
            "__defined_const?" => {
                if let Some(Value::Sym(sid)) = args.first() {
                    // Same fallback chain `Op::LoadConst` uses:
                    // `self.classes` (bare top-level keys + the
                    // qualified keys that key-by-qualified-name
                    // stamps for nested classes), then
                    // `self.constants` (user-assigned constants
                    // including `FOO = 1` and `Foo::Bar = 42`).
                    // Without the constants check, qualified
                    // constant assignment reported "expression"
                    // for `defined?(Foo::Bar)` even though the
                    // value resolved through `Op::LoadConst`.
                    let hit = self.classes.contains_key(sid)
                        || self.constants.contains_key(sid);
                    return Some(Ok(if hit { Value::new_str("constant") } else { Value::Nil }));
                }
                Some(Ok(Value::Nil))
            }
            // `autoload(:Foo, "path")` — Phase 1 of issue #224.
            // Toplevel-only: registers a pending lazy-load on the
            // VM-level registry. First reference to `Foo` via
            // `Op::LoadConst` pops the entry and calls `require`.
            // Class-recv form (`Mod.autoload :Foo, "path"`) is
            // still a no-op stub in dispatch.rs; Phase 2 wires it
            // up to a per-Class registry.
            //
            // Dispatch precedence guard: builtins fire BEFORE the
            // class-body no-recv bridge in `do_call`, so a bare
            // `autoload :X, "p"` inside `class Foo; ... end` would
            // otherwise hit this toplevel handler and incorrectly
            // register on the toplevel scope instead of Foo. We
            // detect that by inspecting the current frame's `self`
            // — if it's a `Value::Class(_)` we're inside a class /
            // module body and defer (`return None`) so the
            // dispatcher continues to the class-arm at
            // `try_dispatch_class_intrinsics` (still a no-op stub
            // for Phase 1, by design).
            //
            // Arity: exactly 2. First arg coerces to Symbol (accept
            // Symbol + String); second arg must be a String. Type
            // mismatches raise the CRuby-shape TypeError. Invalid
            // constant names (lowercase, leading digit, etc.) raise
            // `NameError: wrong constant name <name>` matching
            // CRuby's autoload validation.
            //
            // Under wasm32-wasi the registry is cfg-gated out (no
            // `require` to fire), so the call validates and returns
            // nil — equivalent to the pre-Phase-1 stub behavior on
            // that target.
            "autoload" => {
                if let Some(Value::Class(_)) = self.frames.last().map(|f| f.self_val.clone()) {
                    return None;
                }
                if args.len() != 2 {
                    return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: format!("wrong number of arguments (given {}, expected 2)", args.len()),
                    })));
                }
                let name_sym = match &args[0] {
                    Value::Sym(s) => *s,
                    Value::Str(s) => {
                        // Same `Config::max_symbols` cap as
                        // `String#to_sym` / `parse_send_target` —
                        // without it, untrusted code could grow the
                        // interner unbounded via repeated
                        // `autoload("dyn_#{i}", "x")` calls.
                        let name = s.to_string_lossy();
                        if let Some(max) = self.max_symbols
                            && !self.interner.contains(&name)
                            && self.interner.len() >= max
                        {
                            return Some(Err(self.trap(RubyError::ResourceExhausted {
                                msg: format!("interner exhausted: {} symbols", max),
                            })));
                        }
                        self.interner.intern(&name)
                    }
                    // CRuby's name guard is the id-or-string check, not a
                    // Symbol conversion: it reports the INSPECTED value
                    // ("nil is not a symbol nor a string"). Probed vs 3.4.1;
                    // same family as the Module#autoload/const_set sites.
                    other => {
                        let inspected = other.to_inspect(&self.heap, &self.interner);
                        return Some(Err(self.trap(RubyError::TypeError {
                            msg: format!("{} is not a symbol nor a string", inspected),
                        })));
                    }
                };
                // Validate constant-name shape (uppercase start +
                // alnum/underscore body). CRuby raises NameError on
                // `autoload(:foo, "x")`.
                let name_str = self.interner.resolve(name_sym).clone();
                if !crate::vm::dispatch::is_valid_const_name(&name_str) {
                    return Some(Err(self.trap(RubyError::NameError {
                        msg: format!("wrong constant name {}", name_str),
                    })));
                }
                let path_str = match &args[1] {
                    Value::Str(s) => s.to_string_lossy(),
                    other => return Some(Err(self.trap(RubyError::TypeError {
                        msg: format!("no implicit conversion of {} into String", other.conv_type_name()),
                    }))),
                };
                #[cfg(not(target_os = "wasi"))]
                {
                    self.autoloads_toplevel.insert(name_sym, path_str);
                }
                #[cfg(target_os = "wasi")]
                {
                    let _ = (name_sym, path_str);
                }
                Some(Ok(Value::Nil))
            }
            // `autoload?(:Foo, inherit=true)` — Phase 1 introspection.
            // Returns the registered path String if `:Foo` has a
            // pending toplevel autoload, else nil. The `inherit`
            // arg is accepted for arity parity but not consulted
            // (toplevel scope has no inheritance chain to walk —
            // CRuby's `Object.autoload?(:Foo, false)` would also
            // see a toplevel `autoload :Foo, ...` directly).
            //
            // Same dispatch-precedence guard + const-name validation
            // as the `autoload` arm above — bare `autoload?` inside
            // `class Foo; ... end` defers to the class-arm via
            // `return None`, and invalid constant names raise
            // `NameError: wrong constant name <name>`.
            "autoload?" => {
                if let Some(Value::Class(_)) = self.frames.last().map(|f| f.self_val.clone()) {
                    return None;
                }
                if args.is_empty() || args.len() > 2 {
                    return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: format!("wrong number of arguments (given {}, expected 1..2)", args.len()),
                    })));
                }
                let name_sym = match &args[0] {
                    Value::Sym(s) => *s,
                    Value::Str(s) => {
                        // `Config::max_symbols` cap — same rationale
                        // as the `autoload` arm above. Untrusted
                        // code could otherwise grow the interner via
                        // repeated `autoload?("dyn_#{i}")` probes.
                        let name = s.to_string_lossy();
                        if let Some(max) = self.max_symbols
                            && !self.interner.contains(&name)
                            && self.interner.len() >= max
                        {
                            return Some(Err(self.trap(RubyError::ResourceExhausted {
                                msg: format!("interner exhausted: {} symbols", max),
                            })));
                        }
                        self.interner.intern(&name)
                    }
                    // CRuby's name guard is the id-or-string check, not a
                    // Symbol conversion: it reports the INSPECTED value
                    // ("nil is not a symbol nor a string"). Probed vs 3.4.1;
                    // same family as the Module#autoload/const_set sites.
                    other => {
                        let inspected = other.to_inspect(&self.heap, &self.interner);
                        return Some(Err(self.trap(RubyError::TypeError {
                            msg: format!("{} is not a symbol nor a string", inspected),
                        })));
                    }
                };
                let name_str = self.interner.resolve(name_sym).clone();
                if !crate::vm::dispatch::is_valid_const_name(&name_str) {
                    return Some(Err(self.trap(RubyError::NameError {
                        msg: format!("wrong constant name {}", name_str),
                    })));
                }
                #[cfg(not(target_os = "wasi"))]
                {
                    if let Some(path) = self.autoloads_toplevel.get(&name_sym) {
                        return Some(Ok(Value::new_str(path.clone())));
                    }
                }
                #[cfg(target_os = "wasi")]
                { let _ = name_sym; }
                Some(Ok(Value::Nil))
            }
            "using" => {
                // `using M` — activate M's refinements (Tier-1: global
                // from here on; see Vm::module_refinements). Returns the
                // caller's self in CRuby; nil is close enough (the value
                // is essentially never used) for the subset.
                if let Some(Value::Class(m)) = args.first() {
                    let m = m.clone();
                    self.do_using(&m);
                }
                Some(Ok(Value::Nil))
            }
            "p" | "pp" => {
                // Pin the args across the inspect dispatch: a user
                // `inspect` runs arbitrary code (→ GC) and the arg buffer
                // isn't in the root set.
                {
                    // Redirected `$stdout` (capture_io): render the
                    // inspect lines first (p uses inspect, so the
                    // whole-call forward that puts/print use would
                    // lose it to the target's own to_s), then hand
                    // the text over via a single `write`.
                    let redirect = self.stdio_redirect("$stdout", true);
                    let mut redirected = redirect.as_ref().map(|_| String::new());
                    let pinned: Vec<Value> = args.to_vec();
                    let mut g = PinGuard::new(self);
                    for a in &pinned { g.pin(a.clone()); }
                    for a in &pinned {
                        let s = match g.vm.stringify_for_output(a, true) {
                            Ok(s) => s,
                            Err(t) => return Some(Err(t)),
                        };
                        if let Some(buf) = redirected.as_mut() {
                            buf.push_str(&s);
                            buf.push('\n');
                        } else {
                            let _ = writeln!(g.vm.stdout, "{}", s);
                        }
                    }
                    drop(g);
                    if let (Some(target), Some(buf)) = (redirect, redirected)
                        && let Err(t) = self.forward_stdio_call(target, "write", &[Value::new_str(buf)])
                    {
                        return Some(Err(t));
                    }
                }
                match args {
                    [] => Some(Ok(Value::Nil)),
                    [one] => Some(Ok(one.clone())),
                    many => {
                        // GC rooting: `args` is the `&[Value]` slice
                        // backed by a Vec that `do_call` drained out
                        // of `self.stack` — those Values are NOT in
                        // the root set. Pin each element across
                        // `maybe_gc` + `alloc` so the multi-arg `p` /
                        // `pp` return Array can't reference a freed
                        // slot under STRESS_GC=1. Same shape as the
                        // seven sites from issue #90.
                        let mut g = PinGuard::new(self);
                        let elems: Vec<Value> = many.to_vec();
                        for v in &elems { g.pin(v.clone()); }
                        g.vm.maybe_gc();
                        if let Err(t) = g.vm.check_alloc() { return Some(Err(t)); }
                        let id = g.vm.heap.alloc(HeapObj::Array(elems.into()));
                        Some(Ok(Value::Array(id)))
                    }
                }
            }
            // `Integer(x)` / `Float(x)` / `String(x)` — strict
            // conversion functions. Unlike `to_i` / `to_f` (which
            // are lenient — `"abc".to_i` returns 0), these raise
            // ArgumentError on input that can't be cleanly parsed.
            // The canonical Ruby idiom for "convert or fail loudly",
            // typically wrapped in an inline rescue:
            //   port = Integer(ENV['PORT']) rescue 8080
            // `Integer(x [, base] [, exception: true/false])` —
            // strict conversion. The String path routes through the
            // shared `str2int` scanner (strict mode): whole-string
            // match, `0x/0b/0o/0d` + leading-0-octal prefixes,
            // single `_` between digits, ASCII whitespace at the
            // edges, and EXACT BigInt promotion past i64 range
            // (`Integer("18446744073709551616")` is the precise
            // 2^64, not a wrapped 0). Negative bases are CRuby's
            // "prefix-driven with default |base|" form
            // (`Integer("10", -16)` → 16, `Integer("042", -16)` →
            // octal 34).
            "Integer" => Some(self.kernel_integer(args)),
            "Float" => {
                if args.len() != 1 {
                    return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: format!("wrong number of arguments (given {}, expected 1)", args.len()),
                    })));
                }
                let result = match &args[0] {
                    Value::Float(f) => Ok(Value::Float(*f)),
                    Value::Int(n) => Ok(Value::Float(*n as f64)),
                    Value::Str(s) => {
                        let raw = s.to_string_lossy();
                        let trimmed = raw.trim();
                        match trimmed.parse::<f64>() {
                            Ok(f) => Ok(Value::Float(f)),
                            Err(_) => Err(RubyError::ArgumentError {
                                msg: format!("invalid value for Float(): \"{}\"", raw),
                            }),
                        }
                    }
                    Value::Nil => Err(RubyError::TypeError {
                        msg: "can't convert nil into Float".into(),
                    }),
                    other => Err(RubyError::TypeError {
                        msg: format!("can't convert {} into Float", other.conv_type_name()),
                    }),
                };
                Some(result.map_err(|e| self.trap(e)))
            }
            // `String(x)` — calls `to_s` for any value. Lenient (it
            // doesn't raise on weird input — to_s should always
            // succeed for our built-in types).
            "String" => {
                if args.len() != 1 {
                    return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: format!("wrong number of arguments (given {}, expected 1)", args.len()),
                    })));
                }
                let v = args[0].clone();
                // CRuby's `String(x)` is `x.to_s` (with a to_str precheck) and
                // HONOURS overrides + native `to_s` arms. An Object (user
                // instance, MatchData, …) must dispatch its real `to_s` — the
                // old `to_display` fast path produced `#<Class:0x..>` and
                // dropped overrides (rubocop-ast's SurroundingSpace does
                // `String(token.space_after?)` on a MatchData expecting " ").
                // A real send reaches native dispatch arms `lookup_method_
                // uncached` would miss; primitives keep the fast renderer.
                if let Value::Object(_) = v {
                    let to_s = self.interner.intern("to_s");
                    let pre = self.frames.len();
                    self.stack.push(v);
                    if let Err(e) = self.do_call(to_s, 0, false, u32::MAX) {
                        return Some(Err(e));
                    }
                    if let Err(e) = self.dispatch_until(pre) {
                        return Some(Err(e));
                    }
                    let r = self.stack.pop().unwrap_or(Value::Nil);
                    return Some(Ok(match r {
                        Value::Str(_) => r,
                        // A non-String `to_s` (misbehaving override) → render
                        // natively, matching the lenient spirit.
                        other => Value::new_str(other.to_display(&self.heap, &self.interner)),
                    }));
                }
                let s = v.to_display(&self.heap, &self.interner);
                Some(Ok(Value::new_str(s)))
            }
            // `Rational(num, den)` / `Rational(num)` — Phase C.1
            // public constructor. Accepts Integer num + Integer den
            // (den defaults to 1). gcd-normalizes and sign-normalizes
            // at construction so every live `Value::Rational` is
            // canonical (`den > 0`, `gcd(|num|, den) == 1`).
            "Rational" => {
                if args.is_empty() || args.len() > 2 {
                    return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: format!(
                            "wrong number of arguments (given {}, expected 1..2)",
                            args.len(),
                        ),
                    })));
                }
                // Phase C.4.3: 1-arg Float delegates to the same
                // IEEE-754 decomposition used by `Float#to_r`. NaN /
                // ±Inf surface as FloatDomainError. 2-arg Float
                // forms (`Rational(1.5, 0.5)`) still fall through
                // to TypeError until generic Numeric coercion lands.
                if args.len() == 1
                    && let Value::Float(f) = &args[0]
                {
                    let f = *f;
                    if !f.is_finite() {
                            return Some(Err(self.trap(RubyError::FloatDomainError {
                                msg: crate::vm::numeric::float_domain_label(f).to_string(),
                            })));
                        }
                        return Some(self.float_to_rational_value(
                            f,
                            crate::vm::dispatch::FloatToRationalMode::Lossless,
                        ));
                }
                // Phase C.4.2: accept Int / BigInt / integer-valued
                // Rational.
                #[cfg(feature = "bignum")]
                {
                    use num_bigint::BigInt;
                    use num_traits::One;
                    let to_bigint = |v: &Value, heap: &crate::heap::Heap| -> Result<BigInt, RubyError> {
                        match v {
                            Value::Int(n) => Ok(BigInt::from(*n)),
                            Value::BigInt(id) => Ok(heap.bigint(*id).clone()),
                            Value::Rational(id) => {
                                let r = heap.rational(*id);
                                if r.den.is_one() {
                                    Ok(r.num.clone())
                                } else {
                                    Err(RubyError::TypeError {
                                        msg: format!("can't convert {} into Rational", v.conv_type_name()),
                                    })
                                }
                            }
                            _ => Err(RubyError::TypeError {
                                msg: format!("can't convert {} into Rational", v.conv_type_name()),
                            }),
                        }
                    };
                    let num = match to_bigint(&args[0], &self.heap) {
                        Ok(n) => n,
                        Err(e) => return Some(Err(self.trap(e))),
                    };
                    let den = if args.len() == 2 {
                        match to_bigint(&args[1], &self.heap) {
                            Ok(n) => n,
                            Err(e) => return Some(Err(self.trap(e))),
                        }
                    } else {
                        BigInt::one()
                    };
                    // ZeroDivisionError on den == 0 is centralized in
                    // `make_rational_bigint` (runtime-checked, not just
                    // debug_assert) so callers don't need a parallel guard.
                    Some(self.make_rational_bigint(num, den))
                }
                #[cfg(not(feature = "bignum"))]
                {
                    let to_i64 = |v: &Value, heap: &crate::heap::Heap| -> Result<i64, RubyError> {
                        match v {
                            Value::Int(n) => Ok(*n),
                            Value::Rational(id) => {
                                let r = heap.rational(*id);
                                if r.den == 1 {
                                    Ok(r.num)
                                } else {
                                    Err(RubyError::TypeError {
                                        msg: format!("can't convert {} into Rational", v.conv_type_name()),
                                    })
                                }
                            }
                            _ => Err(RubyError::TypeError {
                                msg: format!("can't convert {} into Rational", v.conv_type_name()),
                            }),
                        }
                    };
                    let num_raw = match to_i64(&args[0], &self.heap) {
                        Ok(n) => n,
                        Err(e) => return Some(Err(self.trap(e))),
                    };
                    let den_raw: i64 = if args.len() == 2 {
                        match to_i64(&args[1], &self.heap) {
                            Ok(n) => n,
                            Err(e) => return Some(Err(self.trap(e))),
                        }
                    } else { 1 };
                    Some(self.make_rational(num_raw, den_raw))
                }
            }
            // `Array(x)` — coerce to Array. CRuby rules:
            //   - `nil` → `[]`
            //   - Array → unchanged
            //   - any other → `[x]`
            // Used by block destructure prologues to handle the
            // common case of "block declared `|head, (a, b)|` but
            // the caller passed nil or a scalar for the second
            // arg" without raising NoMethodError.
            "Array" => {
                if args.len() != 1 {
                    return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: format!("wrong number of arguments (given {}, expected 1)", args.len()),
                    })));
                }
                match &args[0] {
                    Value::Nil => {
                        self.maybe_gc(); // allow: gc-rooting — allocates an empty Array (`Vec::new()`); no Value held across the alloc window.
                        if let Err(t) = self.check_alloc() { return Some(Err(t)); }
                        let id = self.heap.alloc(crate::heap::HeapObj::Array(Vec::new().into()));
                        Some(Ok(Value::Array(id)))
                    }
                    Value::Array(_) => Some(Ok(args[0].clone())),
                    // `Array(hash)` → pair-array via `Hash#to_a`.
                    // CRuby converts `{a: 1, b: 2}` to
                    // `[[:a, 1], [:b, 2]]`. (TRY_RUNS layer #25
                    // pre-existing gap surfaced by the fixture.)
                    Value::Hash(hid) => {
                        // Pattern mirrors `Hash#to_a` (vm/hash.rs):
                        // per-pair `maybe_gc` and pinning of the
                        // freshly-allocated pair_id so a future
                        // refactor making `heap.alloc` GC-triggering
                        // doesn't sweep accumulated entries. K/V
                        // sources are pinned up front; each new
                        // pair_id is pinned as it's built. Layer #25
                        // code-review follow-up.
                        let pairs: Vec<(Value, Value)> = self.heap.hash(*hid).iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect();
                        let mut g = PinGuard::new(self);
                        for (k, v) in &pairs {
                            g.pin(k.clone());
                            g.pin(v.clone());
                        }
                        let mut entries: Vec<Value> = Vec::with_capacity(pairs.len());
                        for (k, v) in pairs {
                            g.vm.maybe_gc();
                            if let Err(t) = g.vm.check_alloc() { return Some(Err(t)); }
                            let pair_id = g.vm.heap.alloc(crate::heap::HeapObj::Array(vec![k, v].into()));
                            let pair_val = Value::Array(pair_id);
                            g.pin(pair_val.clone());
                            entries.push(pair_val);
                        }
                        g.vm.maybe_gc();
                        if let Err(t) = g.vm.check_alloc() { return Some(Err(t)); }
                        let id = g.vm.heap.alloc(crate::heap::HeapObj::Array(entries.into()));
                        Some(Ok(Value::Array(id)))
                    }
                    // `Array(range)` expands the Range to its elements
                    // (`Array(1..3) == [1, 2, 3]`). Range#to_a is a native
                    // method (not in the method table), so the to_ary/to_a
                    // table fallback below misses it — dispatch by name.
                    Value::Range(_) => {
                        let recv = args[0].clone();
                        let pre = self.frames.len();
                        self.stack.push(recv);
                        let to_a_id = self.interner.intern("to_a");
                        if let Err(t) = self.do_call(to_a_id, 0, false, u32::MAX) {
                            return Some(Err(t));
                        }
                        if let Err(t) = self.dispatch_until(pre) { return Some(Err(t)); }
                        Some(Ok(self.stack.pop().unwrap_or(Value::Nil)))
                    }
                    _ => {
                        // CRuby's `Array(obj)` coerces via `to_ary`
                        // then `to_a` before wrapping in `[obj]`
                        // (rb_Array: rb_check_array_type →
                        // rb_check_to_array → `[val]`). A user object
                        // exposing either (e.g. Rack::Response#to_a →
                        // `[status, headers, body]`) is expanded; this
                        // also backs `[*obj]` / `a, b = *obj`, which
                        // desugar through `Array(obj)`. We consult the
                        // method TABLE only — native primitives (Range
                        // etc.) aren't there, so they keep the `[obj]`
                        // fallback, a pre-existing gap left untouched.
                        let recv = args[0].clone();
                        for conv in ["to_ary", "to_a"] {
                            let mid = self.interner.intern(conv);
                            let m = match self.class_of(&recv) {
                                Value::Class(cls) => self.lookup_method_uncached(&cls, mid),
                                _ => None,
                            };
                            let Some(m) = m else { continue };
                            let pre = self.frames.len();
                            let mut g = PinGuard::new(self);
                            g.pin(recv.clone());
                            if let Err(t) = g.vm.invoke_method(m, recv.clone(), vec![]) {
                                return Some(Err(t));
                            }
                            if let Err(t) = g.vm.dispatch_until(pre) {
                                return Some(Err(t));
                            }
                            // Only an Array result is accepted; nil (or
                            // a non-Array from a misbehaving override)
                            // falls through to the next conversion and
                            // ultimately the `[obj]` wrap — lenient vs
                            // CRuby's TypeError on a non-Array, never
                            // hit by real to_ary/to_a implementations.
                            let got_array = matches!(g.vm.stack.last(), Some(Value::Array(_)));
                            let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                            if got_array {
                                return Some(Ok(r));
                            }
                        }
                        // GC rooting: `args` was drained out of
                        // `self.stack` in `do_call`, so `args[0]` is
                        // a Rust local — NOT in the root set
                        // (stack / pinned / frame locals / globals
                        // / constants). Under STRESS_GC=1 every
                        // alloc triggers mark+sweep, so the slot
                        // referenced by `args[0]` would be reaped
                        // between `maybe_gc` and `heap.alloc` and
                        // the new one-element Array would point at
                        // a recycled slot, surfacing as
                        // `class_of called on non-Object slot`.
                        // See issue #90, site #8. Mirrors the fix
                        // applied at the 6 prior sites in commits
                        // 86db73d / f2c3538 / 5946caa.
                        let mut g = PinGuard::new(self);
                        let elt = args[0].clone();
                        g.pin(elt.clone());
                        g.vm.maybe_gc();
                        if let Err(t) = g.vm.check_alloc() { return Some(Err(t)); }
                        let id = g.vm.heap.alloc(crate::heap::HeapObj::Array(vec![elt].into()));
                        Some(Ok(Value::Array(id)))
                    }
                }
            }
            // `Kernel#Hash(arg)` — nil or `[]` → `{}`; a Hash → itself;
            // an object with `to_hash` → its result; anything else
            // raises TypeError. (CRuby is deliberately narrow here:
            // unlike Array(), it does NOT wrap arbitrary values.)
            "Hash" => {
                if args.len() != 1 {
                    return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: format!("wrong number of arguments (given {}, expected 1)", args.len()),
                    })));
                }
                let empty_array = matches!(&args[0], Value::Array(aid) if self.heap.array(*aid).is_empty());
                match &args[0] {
                    Value::Hash(_) => Some(Ok(args[0].clone())),
                    Value::Nil => {
                        self.maybe_gc(); // allow: gc-rooting — empty Hash holds no Value; args[0] (Nil) is not used across the alloc
                        if let Err(t) = self.check_alloc() { return Some(Err(t)); }
                        let id = self.heap.alloc(crate::heap::HeapObj::Hash(
                            crate::heap::HashObj::with_pairs(Vec::new()),
                        ));
                        Some(Ok(Value::Hash(id)))
                    }
                    _ if empty_array => {
                        self.maybe_gc();
                        if let Err(t) = self.check_alloc() { return Some(Err(t)); }
                        let id = self.heap.alloc(crate::heap::HeapObj::Hash(
                            crate::heap::HashObj::with_pairs(Vec::new()),
                        ));
                        Some(Ok(Value::Hash(id)))
                    }
                    other => {
                        // CRuby prints the CLASS name in Hash()'s "can't convert X
                        // into Hash" (probed: Hash(true) → "can't convert TrueClass
                        // into Hash"), and type_name's Bool arm is the non-Ruby
                        // "Boolean" — use the class-name helper instead.
                        let tn = crate::vm::numeric::class_name_for_error(other).to_string();
                        let recv = args[0].clone();
                        let mid = self.interner.intern("to_hash");
                        let m = match self.class_of(&recv) {
                            Value::Class(cls) => self.lookup_method_uncached(&cls, mid),
                            _ => None,
                        };
                        if let Some(m) = m {
                            let pre = self.frames.len();
                            let mut g = PinGuard::new(self);
                            g.pin(recv.clone());
                            if let Err(t) = g.vm.invoke_method(m, recv.clone(), vec![]) {
                                return Some(Err(t));
                            }
                            if let Err(t) = g.vm.dispatch_until(pre) { return Some(Err(t)); }
                            let r = g.vm.stack.pop().unwrap_or(Value::Nil);
                            if matches!(r, Value::Hash(_)) {
                                return Some(Ok(r));
                            }
                        }
                        Some(Err(self.trap(RubyError::TypeError {
                            msg: format!("can't convert {} into Hash", tn),
                        })))
                    }
                }
            }
            "print" => {
                if let Some(target) = self.stdio_redirect("$stdout", true) {
                    return Some(self.forward_stdio_call(target, "print", args));
                }
                let pinned: Vec<Value> = args.to_vec();
                let mut g = PinGuard::new(self);
                for a in &pinned { g.pin(a.clone()); }
                for a in &pinned {
                    let s = match g.vm.stringify_for_output(a, false) {
                        Ok(s) => s,
                        Err(t) => return Some(Err(t)),
                    };
                    let _ = write!(g.vm.stdout, "{}", s);
                }
                Some(Ok(Value::Nil))
            }
            // `Kernel#sprintf` / `Kernel#format` — printf-style
            // formatter. Same engine `String#%` uses (`ruby_sprintf`
            // in vm/sprintf.rs), just routed through the no-recv
            // Kernel dispatch. `format` is the documented alias.
            // First arg is the format String; remaining args are
            // positional substitutions.
            //
            // `__time_now_raw` — Tier 1 wall-clock primitive used by
            // the `preamble/time.rb` Time class. Returns
            // `[epoch_seconds, nanoseconds]` (2-element Array) if the
            // host injected `Config::time_now`; otherwise raises
            // `RuntimeError`. Deterministic-by-default per ADR 0017
            // Rule 1: the CLI binary opts in by injecting
            // `SystemTime::now()`; library embedders that don't want
            // the host clock exposed leave the field `None` and any
            // `Time.now` call raises.
            "__time_now_raw" => {
                if !args.is_empty() {
                    return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: format!(
                            "wrong number of arguments (given {}, expected 0)",
                            args.len(),
                        ),
                    })));
                }
                let Some(src) = self.time_now.clone() else {
                    return Some(Err(self.trap(RubyError::RuntimeError {
                        msg: "Time.now requires `Config::time_now` injection — \
                              the embedding host hasn't enabled the wall-clock \
                              capability (Tier 1 deterministic default)".into(),
                    })));
                };
                let (sec, nsec) = src();
                self.maybe_gc();
                if let Err(e) = self.check_alloc() {
                    return Some(Err(e));
                }
                let arr = vec![Value::Int(sec), Value::Int(nsec as i64)];
                let id = self.heap.alloc(HeapObj::Array(arr.into()));
                Some(Ok(Value::Array(id)))
            }
            // `__rubyrs_time_parse_iso` — pure-computation fast path
            // for `Time.parse`'s ISO-8601-ish grammar (the
            // preamble/time.rb parser interpreted ~15µs/call; Jekyll
            // parses ~1 unique date string per document). Returns the
            // epoch-seconds Int for inputs it is CERTAIN about, Nil
            // to decline — the preamble then falls back to its Ruby
            // parser, so accepted-input semantics can't drift: the
            // helper only accepts strictly-digit fields and computes
            // the IDENTICAL days_from_civil arithmetic; anything
            // looser (to_i prefix coercion, junk suffixes, huge
            // years) declines to Ruby. No capability gate: unlike
            // `__time_now_raw` this reads no clock (deterministic).
            "__rubyrs_time_parse_iso" => {
                let [Value::Str(s)] = args else {
                    return Some(Ok(Value::Nil));
                };
                let bytes = s.content.borrow();
                let Ok(text) = std::str::from_utf8(&bytes) else {
                    return Some(Ok(Value::Nil));
                };
                Some(Ok(match time_parse_iso(text) {
                    Some(total) => Value::Int(total),
                    None => Value::Nil,
                }))
            }
            // `Kernel#sleep(seconds)` — host-injected via
            // `Config::sleep_for` (ADR 0017 Rule 1
            // closure pattern, same shape as `time_now`).
            // Library / embed default is `None` → trap, so
            // sandbox hosts can keep deterministic timing
            // without opt-in. CLI binary wires this to
            // `std::thread::sleep` so `rubyrs script.rb`
            // matches CRuby.
            //
            // Accepts `Integer` or `Float` seconds; both
            // converted to `Duration` (Float → nanos via
            // `from_secs_f64`). CRuby returns the integer
            // seconds actually slept — we return the
            // requested seconds (rounded down) as a
            // conservative lower bound since
            // `std::thread::sleep` doesn't undersleep.
            //
            // Negative durations: CRuby raises
            // `ArgumentError("time interval must not be
            // negative")`; we match.
            // `__rubyrs_kernel_sleep` is the escape hatch the
            // cooperative scheduler's Ruby-level `Object#sleep`
            // override (preamble/thread.rb) uses to reach the native
            // sleep without re-entering the override check below —
            // otherwise override → native → override would loop.
            "sleep" | "__rubyrs_kernel_sleep" => {
                // A user/stub override on self's class chain wins —
                // bare `sleep(10)` in CRuby is an ordinary Kernel
                // method, and minitest's `self.stub :sleep, nil`
                // installs one on the test instance's eigenclass.
                // Same cold-path gate shape as Op::Raise's; the
                // kernel-alias forwarder (the saved original) is
                // excluded so the restore cycle can't loop.
                if name == "sleep" {
                    let sleep_sym = self.interner.intern("sleep");
                    let self_val = self.frames.last().map(|f| f.self_val.clone());
                    if let Some(Value::Object(oid)) = &self_val {
                        let cls = self.heap.class_of(*oid);
                        if let Some(m) = self.lookup_method_uncached(&cls, sleep_sym)
                            && !self.protos[m.proto_idx].name.starts_with("<kernel-alias-forwarder")
                        {
                            let self_val = self_val.expect("checked above");
                            let pre_frames = self.frames.len();
                            if let Err(t) = self
                                .invoke_method(m, self_val, args.to_vec())
                                .and_then(|()| self.dispatch_until(pre_frames))
                            {
                                return Some(Err(t));
                            }
                            let v = self.stack.pop().unwrap_or(Value::Nil);
                            return Some(Ok(v));
                        }
                    }
                }
                // ADR 0025 Phase 3 (CRuby-faithful semantics):
                //   - no args + signal handler installed →
                //     sleep until SIGINT; raise Interrupt.
                //   - no args + NO signal handler →
                //     ArgumentError (would deadlock — no wake).
                //   - Integer/Float secs → sleep up to secs;
                //     interrupted by SIGINT raises Interrupt
                //     mid-call; otherwise returns Integer
                //     seconds requested.
                let secs_opt = match args {
                    [] => None,
                    [Value::Int(n)] => Some(*n as f64),
                    [Value::Float(f)] => Some(*f),
                    // v7 round-3 parity: accept Rational. CRuby
                    // accepts any Numeric (Rational, BigDecimal,
                    // etc.); we cover the common one here.
                    // BigDecimal not yet implemented; BigInt
                    // converts via to_f only when secs fits f64
                    // — fall through to TypeError otherwise.
                    [Value::Rational(id)] => {
                        let r = self.heap.rational(*id);
                        Some(crate::heap::rational_to_f64(r))
                    }
                    [other] => return Some(Err(self.trap(RubyError::TypeError {
                        msg: format!(
                            "sleep duration must be Integer / Float / Rational, got {}",
                            other.type_name(),
                        ),
                    }))),
                    _ => return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: format!(
                            "wrong number of arguments (given {}, expected 0..1)",
                            args.len(),
                        ),
                    }))),
                };
                if let Some(s) = secs_opt
                    && s < 0.0
                {
                    return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: "time interval must not be negative".into(),
                    })));
                }
                let Some(src) = self.sleep_for.clone() else {
                    return Some(Err(self.trap(RubyError::RuntimeError {
                        msg: "Kernel#sleep requires `Config::sleep_for` injection — \
                              the embedding host hasn't enabled the wall-clock \
                              sleep capability (Tier 1 deterministic default)".into(),
                    })));
                };
                let dur_opt = secs_opt.map(std::time::Duration::from_secs_f64);
                // Sleep-forever (no args) requires the signal
                // handler to wake us; check that it's wired before
                // we commit to the blocking call. Without
                // `install_signal_handler: true`, the flag never
                // flips and `sleep_forever` would deadlock.
                #[cfg(unix)]
                if dur_opt.is_none() {
                    // Heuristic: if SHARED_FLAG hasn't been
                    // populated, no Runtime has opted in. (A
                    // separate Runtime in this process opting in
                    // counts — its handler will store into
                    // SHARED_FLAG; but THIS Vm has a dedicated
                    // flag that doesn't share, so it would still
                    // deadlock. Match against that case by
                    // checking Arc identity.) Practical net:
                    // require the same Runtime that's calling
                    // sleep to have opted in.
                    if !crate::signals::is_shared_flag(&self.interrupt_pending) {
                        return Some(Err(self.trap(RubyError::ArgumentError {
                            msg: "sleep with no arguments requires \
                                  `Config::install_signal_handler: true` (otherwise the \
                                  call would deadlock — nothing can wake it)".into(),
                        })));
                    }
                }
                let elapsed = src(dur_opt, &self.interrupt_pending);
                // If the closure returned early because the flag
                // flipped, raise Interrupt directly from the
                // builtin — matches CRuby (sleep does NOT return
                // on interrupt). The Phase 2 safe-point check
                // would catch it on the next op too, but
                // raising here keeps the trap site at the
                // canonical CRuby location.
                #[cfg(unix)]
                if self.interrupt_pending.load(std::sync::atomic::Ordering::Relaxed) {
                    self.interrupt_pending.store(false, std::sync::atomic::Ordering::Relaxed);
                    let exc = match crate::vm::raise::build_interrupt_exception(self) {
                        Some(v) => v,
                        None => return Some(Err(self.trap(RubyError::Interrupt {
                            msg: "interrupt".to_string(),
                        }))),
                    };
                    if let Err(trap) = self.unwind_with_exception(exc) {
                        return Some(Err(trap));
                    }
                    self.suppress_call_result_push = true;
                    return Some(Ok(Value::Nil));
                }
                // Normal completion. Return Integer seconds —
                // for no-args case (only reachable when the flag
                // was set, but we just cleared+raised above) we
                // fall through to here with elapsed; clamp to
                // 0 to be defensive.
                let returned = secs_opt.map(|s| s as i64)
                    .unwrap_or_else(|| elapsed.as_secs() as i64);
                Some(Ok(Value::Int(returned)))
            }
            // ADR 0025 Phase 0.5b: `Kernel#exit` / `exit!` /
            // `abort`. Three shapes:
            //   `exit(status = true)` — raises SystemExit; ensure
            //     blocks fire, at_exit handlers run (Phase 4),
            //     embedder reads status.
            //   `exit!(status = false)` — IMMEDIATE process exit
            //     via the host-injected `process_exit` closure.
            //     SKIPS ensure + at_exit. Requires Tier 1
            //     capability (Config::process_exit) per ADR 0017
            //     Rule 1.
            //   `abort(msg = nil)` — write msg to stderr (if
            //     given), then `exit(1)`.
            //
            // `exit` shares the "construct SystemExit + unwind"
            // path with `abort` since both end in a SystemExit
            // raise; the helper is inlined to avoid an additional
            // module-level fn.
            "exit" => {
                let status = match parse_exit_status(args) {
                    Ok(s) => s,
                    Err(early) => return early,
                };
                raise_system_exit(self, status, "exit")
            }
            "abort" => {
                // Optional message argument prints to stderr; in
                // either case, exit(1) follows.
                //
                // ADR 0025 deferred follow-up: no-args `abort`
                // consults `$!` — when called mid-rescue, CRuby
                // writes "<exc-class>: <exc-message>" to stderr
                // before raising SystemExit(1). Pre-fix the
                // no-args path was silent.
                //
                // Tier-1 2c: now writes to `Vm::stderr` (was
                // stdout before the stderr channel landed).
                if args.len() > 1 {
                    return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: format!(
                            "wrong number of arguments (given {}, expected 0..1)",
                            args.len(),
                        ),
                    })));
                }
                let msg = match args.first() {
                    Some(Value::Str(s)) => Some(s.to_string_lossy()),
                    Some(other) => return Some(Err(self.trap(RubyError::TypeError {
                        msg: format!(
                            "no implicit conversion of {} into String",
                            other.conv_type_name(),
                        ),
                    }))),
                    None => {
                        // No-args: consult `$!`. If it's an
                        // Object with a class + message, format
                        // CRuby-style. If nil, no message.
                        match self.globals.get(&self.sym_bang).cloned() {
                            Some(Value::Object(id)) => {
                                let cls_name = self.heap.real_class_of(id).name.clone();
                                let msg_sym = self.interner.intern("@message");
                                let inner = self.heap.instance(id).ivar_get(msg_sym).cloned()
                                    .map(|v| v.to_display(&self.heap, &self.interner))
                                    .unwrap_or_default();
                                Some(format!("{cls_name}: {inner}"))
                            }
                            _ => None,
                        }
                    }
                };
                if let Some(m) = msg.as_ref() {
                    if m.ends_with('\n') {
                        let _ = write!(self.stderr, "{m}");
                    } else {
                        let _ = writeln!(self.stderr, "{m}");
                    }
                }
                raise_system_exit(self, 1, msg.as_deref().unwrap_or("exit"))
            }
            // `__rubyrs_stdout_write` / `__rubyrs_stderr_write` —
            // raw byte sinks behind the preamble's STDOUT/STDERR IO
            // objects (preamble/process.rb). One String arg, written
            // verbatim (no newline normalisation — that's IO#puts's
            // job in Ruby); returns nil. Raw bytes (not to_display)
            // so binary-ish output round-trips like CRuby's
            // IO#write.
            "__rubyrs_stdout_write" | "__rubyrs_stderr_write" => {
                if let [Value::Str(sv)] = args {
                    let bytes = sv.borrow();
                    if name == "__rubyrs_stdout_write" {
                        let _ = self.stdout.write_all(&bytes);
                    } else {
                        let _ = self.stderr.write_all(&bytes);
                    }
                } else {
                    return Some(Err(self.trap(RubyError::TypeError {
                        msg: "stdio write expects a single String".to_string(),
                    })));
                }
                Some(Ok(Value::Nil))
            }
            "__rubyrs_exe_path" => {
                // Path to the running rubyrs executable. RbConfig uses
                // it to populate bindir / ruby_install_name and
                // `RbConfig.ruby` HONESTLY (rubyrs IS the interpreter),
                // rather than inventing a path — rake's file_utils.rb
                // computes `RUBY = File.join(bindir, ruby_install_name +
                // EXEEXT)` at load. `nil` if the OS can't report it.
                match std::env::current_exe() {
                    Ok(p) => Some(Ok(Value::new_str(p.to_string_lossy().into_owned()))),
                    Err(_) => Some(Ok(Value::Nil)),
                }
            }
            "warn" => {
                // `Kernel#warn(*msgs, uplevel: nil, category: nil)`
                // writes each message + "\n" to `Vm::stderr`. CRuby
                // honours two keywords:
                //   - `uplevel:` prefixes the FIRST message with the
                //     location `uplevel` frames up from the warn call
                //     site, as `"path:line: warning: "` (just
                //     `"warning: "` when that points beyond the stack);
                //   - `category: :deprecated` is SUPPRESSED by default
                //     (`Warning[:deprecated]` is false without
                //     `-W:deprecated`), so the message is dropped.
                // rubyrs flattens kwargs into a trailing positional
                // Hash, so peel a trailing Hash whose keys are all
                // `:uplevel`/`:category` (a Hash with other keys is a
                // real message — `warn({a: 1})`).
                //
                // USER OVERRIDE WINS: a `warn` defined on self's
                // method chain (or, for a Class/Module self, its
                // singleton chain) shadows Kernel#warn for bare
                // calls — CRuby method lookup runs before
                // Kernel's. rack's test helper captures
                // deprecation warnings by
                // `define_singleton_method(:warn)` on Rack::Utils
                // and the module's own bodies call bare `warn`;
                // without this fall-through the capture never
                // sees them. `return None` defers to the normal
                // dispatch walk (same mechanism as the class-body
                // `autoload` deferral above).
                if self.bare_builtin_user_override("warn") {
                    return None;
                }
                let mut msgs: &[Value] = args;
                let mut uplevel: Option<i64> = None;
                let mut category: Option<String> = None;
                if let Some(Value::Hash(hid)) = args.last() {
                    let pairs = self.heap.hash(*hid).to_vec();
                    let all_kw = !pairs.is_empty()
                        && pairs.iter().all(|(k, _)| matches!(k, Value::Sym(s)
                            if matches!(&**self.interner.resolve(*s), "uplevel" | "category")));
                    if all_kw {
                        for (k, v) in &pairs {
                            if let Value::Sym(s) = k {
                                match &**self.interner.resolve(*s) {
                                    "uplevel" => {
                                        if let Value::Int(n) = v { uplevel = Some(*n); }
                                    }
                                    "category" => {
                                        if let Value::Sym(cs) = v {
                                            category = Some(self.interner.resolve(*cs).to_string());
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        msgs = &args[..args.len() - 1];
                    }
                }
                if category.as_deref() == Some("deprecated") {
                    return Some(Ok(Value::Nil)); // suppressed by default
                }
                if msgs.is_empty() {
                    return Some(Ok(Value::Nil));
                }
                // Resolve the `uplevel:` location prefix (first line only).
                let prefix: Option<String> = uplevel.map(|lvl| {
                    let lvl = lvl.max(0) as usize;
                    let loc = self.frames.len().checked_sub(1 + lvl).map(|i| {
                        let f = &self.frames[i];
                        let proto = &self.protos[f.proto_idx];
                        let op_ip = if f.ip == 0 { 0 } else { f.ip - 1 };
                        let span = proto
                            .op_spans
                            .get(op_ip)
                            .copied()
                            .unwrap_or(crate::error::Span::ZERO);
                        let line = match self.sources.get(proto.filename.as_ref()) {
                            Some(src) => crate::error::line_col(src, span.byte_offset).0,
                            None => 0,
                        };
                        format!("{}:{}", proto.filename, line)
                    });
                    match loc {
                        Some(l) => format!("{l}: warning: "),
                        None => "warning: ".to_string(),
                    }
                });
                let mut buf = String::new();
                for (i, arg) in msgs.iter().enumerate() {
                    if i == 0 && let Some(p) = &prefix {
                        buf.push_str(p);
                    }
                    let s = arg.to_display(&self.heap, &self.interner);
                    buf.push_str(&s);
                    if !s.ends_with('\n') {
                        buf.push('\n');
                    }
                }
                if let Some(target) = self.stdio_redirect("$stderr", true) {
                    // Forward as a single write — same shape as the
                    // redirected `p`.
                    return Some(self.forward_stdio_call(target, "write", &[Value::new_str(buf)]));
                }
                let _ = write!(self.stderr, "{buf}");
                Some(Ok(Value::Nil))
            }
            "exit!" => {
                let status = match parse_exit_status(args) {
                    Ok(s) => s,
                    Err(early) => return early,
                };
                let Some(src) = self.process_exit.clone() else {
                    return Some(Err(self.trap(RubyError::RuntimeError {
                        msg: "Kernel#exit! requires `Config::process_exit` injection — \
                              the embedding host hasn't enabled immediate process \
                              termination (Tier 1 deterministic default)".into(),
                    })));
                };
                // The closure typically calls std::process::exit
                // and never returns. If it DOES return (test host
                // intercepts), fall through with Nil — exit! has
                // no Ruby-level return value.
                src(status);
                Some(Ok(Value::Nil))
            }
            // ADR 0025 Phase 4a: `Signal.trap(sig, handler)` →
            // previous handler. The `Signal` Ruby module
            // (preamble) calls this host fn after normalizing
            // its block argument. Three-arg shape:
            //   `__rubyrs_signal_trap(sig, handler, block)`
            // where exactly one of `handler` or `block` is
            // non-nil (the Signal module enforces this; if both
            // are nil, the host fn returns the current handler
            // unchanged — useful for "what's set?" queries).
            //
            // Accepted handler inputs (CRuby parity):
            //   "DEFAULT" / :DEFAULT          → Default state
            //   "IGNORE"  / :IGNORE / "SIG_IGN" → Ignore state
            //   Proc / block                  → Block(ObjId)
            //
            // Returns previous handler in the same shape:
            //   Default → "DEFAULT" String
            //   Ignore  → "IGNORE"  String
            //   Block(id) → Value::Block(id)
            // ADR 0025 Phase 4c: `Kernel#at_exit` without an
            // attached block reaches this arm via `do_call`'s
            // `builtin_call` invocation. CRuby raises LocalJumpError;
            // we match. The WITH-block form is intercepted in
            // `do_call_block`'s no_recv arm (alongside `lambda` /
            // `proc`) where the block ObjId is in scope.
            // Marshal round-trip primitives (see Vm::marshal_registry).
            // `__rubyrs_marshal_stash(obj)` → token String; the token
            // doubles as valid YAML (a comment line after an empty
            // hash) so disk-written dumps still degrade gracefully
            // through SafeYAML fallback chains.
            // `__rubyrs_math(op, x[, y])` — single host entry for the
            // f64 math surface behind preamble/math (Math.sqrt/log/
            // sin/...). Domain handling stays in Ruby (the preamble
            // raises Math::DomainError on CRuby's contract); this
            // primitive just computes, returning NaN/Infinity as
            // the raw operation does.
            // Dispatch-form raise: bare `raise` compiles straight to
            // Op::Raise, but the kernel-alias forwarder (a stub's
            // saved original) and `send(:raise, ...)`-style paths
            // re-enter through the method namespace. Same three
            // shapes as the compiler intercept; 2+ args set the
            // message directly on the normalized instance (the
            // initialize dispatch is skipped — standard error
            // classes only carry @message).
            "raise" | "fail" => {
                // User override wins (rack Files defines a private
                // `fail(status, body)` returning a response triple
                // — bare `fail(404, ...)` must dispatch there, not
                // raise). Same deferral as `warn` above; the
                // existing raise_as_method fixture pins the
                // eigenclass-stub flavour of this.
                if self.bare_builtin_user_override(name) {
                    return None;
                }
                let v = match args.len() {
                    0 => Value::Nil,
                    1 => args[0].clone(),
                    _ => {
                        let exc = self.normalize_exception(args[0].clone());
                        if let (Value::Object(id), Value::Str(_)) = (&exc, &args[1]) {
                            let msg_id = self.interner.intern("@message");
                            self.heap.instance_mut(*id).ivar_set(msg_id, args[1].clone());
                        }
                        exc
                    }
                };
                if let Err(t) = self.do_raise_value(v) {
                    return Some(Err(t));
                }
                // Unwind already retargeted the frame; nothing to
                // push (the handler's stack depth is authoritative).
                self.suppress_call_result_push = true;
                Some(Ok(Value::Nil))
            }
            // `Kernel#system(*args)` — subprocess execution behind
            // the `allow_process_spawn` capability (ADR 0017: CLI
            // opts in, library embeds keep the deterministic nil =
            // "command could not be executed", which probing
            // callers — minitest's diff-tool discovery — treat as
            // feature-absent). Two CRuby shapes: a single string
            // runs through the shell; multiple args exec directly.
            // stdout/stderr inherit (a same-file `system "diff" f f`
            // probe prints nothing). Spawn failure (ENOENT) → nil.
            // Fork support probe — the preamble gates the
            // Kernel#fork / Process.fork definitions on this, so
            // `respond_to?(:fork)` is false exactly where CRuby's
            // is (Windows) plus where the spawn capability is off
            // (minitest then takes its documented skip path).
            "__rubyrs_fork_supported?" => {
                #[cfg(all(unix, not(target_os = "wasi")))]
                let supported = self.allow_process_spawn;
                #[cfg(not(all(unix, not(target_os = "wasi"))))]
                let supported = false;
                Some(Ok(Value::Bool(supported)))
            }
            // `fork { ... }` body runner (block arrives as an
            // explicit Proc arg via the preamble wrapper — the
            // builtin channel has no block slot). Parent returns
            // the child pid; the CHILD runs the block to
            // completion, drains its inherited at_exit handlers
            // (the snapshot semantics match what a mid-run fork
            // inherits — minitest's nested autorun at_exit), and
            // hard-exits without returning to the Ruby caller.
            #[cfg(all(unix, not(target_os = "wasi")))]
            "__rubyrs_fork_block" => {
                if !self.allow_process_spawn {
                    return Some(Err(self.trap(RubyError::HostException {
                        class_name: "NotImplementedError".to_string(),
                        message: "fork is not available (process spawn capability is off)".to_string(),
                    })));
                }
                let blk = match args.first() {
                    Some(Value::Block(bid)) => *bid,
                    _ => return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: "fork requires a block in rubyrs (Tier-1 subset)".into(),
                    }))),
                };
                // Flush BOTH host stdio buffers — the child is a
                // process copy; unflushed bytes would print twice.
                {
                    use std::io::Write;
                    let _ = std::io::stdout().flush();
                    let _ = std::io::stderr().flush();
                }
                let pid = unsafe { libc::fork() };
                if pid < 0 {
                    return Some(Err(self.trap(RubyError::HostException {
                        class_name: "Errno::EAGAIN".to_string(),
                        message: "Resource temporarily unavailable - fork(2)".to_string(),
                    })));
                }
                if pid > 0 {
                    // Parent: child pid, Ruby-side.
                    return Some(Ok(Value::Int(pid as i64)));
                }
                // ---- CHILD ----
                // `$$` / Process.pid must observe the NEW pid —
                // minitest's autorun at_exit guards on
                // `Process.pid != install_pid`.
                self.pid = Some(unsafe { libc::getpid() } as i64);
                // The child NEVER returns to its Ruby caller — the
                // block is its whole program. Truncate the
                // inherited frame/operand stacks BEFORE running it,
                // or an `exit` inside the block unwinds through the
                // parent's residual frames and gets swallowed by an
                // enclosing rescue (minitest's capture_exceptions
                // turned `fork { exit 42 }` into status 0).
                self.frames.clear();
                self.stack.clear();
                self.pinned.clear();
                self.clear_control_flow_signals();
                // POSIX fork semantics: only the forking thread
                // survives in the child. If the fork happened while
                // green-thread fibers existed (cooperative scheduler,
                // preamble/thread.rb), the child must not carry the
                // parent's fiber execution state — a stray
                // current_fiber_id would make Fiber.current /
                // Fiber.yield in the child act as if it were inside a
                // fiber whose frames were just cleared. The Ruby-level
                // scheduler tables are reset by the preamble fork
                // wrapper (`Thread.__coop_after_fork!`).
                #[cfg(feature = "_fiber")]
                {
                    self.current_fiber_id = None;
                    self.fiber_yield_pending = None;
                    self.fiber_stash_stack.clear();
                }
                let mut status: i32 = {
                    let r = self.invoke_block(blk, Vec::new())
                        .and_then(|()| self.dispatch_until(0))
                        .map(|()| { self.stack.pop(); });
                    match r {
                        Ok(()) => 0,
                        Err(t) => self.fork_child_status_from_trap(&t),
                    }
                };
                // Drain inherited at_exit handlers LIFO (simplified
                // twin of Runtime::drain_at_exit_handlers — no
                // catch_unwind layer; a SystemExit raised by a
                // handler REPLACES the status, CRuby semantics).
                let handlers: Vec<crate::value::ObjId> =
                    self.at_exit_handlers.drain(..).rev().collect();
                for h in handlers {
                    self.clear_control_flow_signals();
                    self.frames.clear();
                    self.stack.clear();
                    let r = self.invoke_block(h, Vec::new())
                        .and_then(|()| self.dispatch_until(0))
                        .map(|()| { self.stack.pop(); });
                    if let Err(t) = r {
                        status = self.fork_child_status_from_trap(&t);
                    }
                }
                {
                    use std::io::Write;
                    let _ = std::io::stdout().flush();
                    let _ = std::io::stderr().flush();
                }
                std::process::exit(status);
            }
            #[cfg(all(unix, not(target_os = "wasi")))]
            "__rubyrs_waitpid" => {
                let pid = match args.first() {
                    Some(Value::Int(n)) => *n as i32,
                    _ => return Some(Err(self.trap(RubyError::TypeError {
                        msg: "waitpid pid must be an Integer".into(),
                    }))),
                };
                // Flags pass through to waitpid(2). WNOHANG (1) is the
                // cooperative scheduler's probe: Process.wait from a
                // green thread polls with WNOHANG + parks between
                // attempts instead of blocking the whole VM.
                let flags = match args.get(1) {
                    Some(Value::Int(n)) => *n as i32,
                    Some(Value::Nil) | None => 0,
                    _ => return Some(Err(self.trap(RubyError::TypeError {
                        msg: "waitpid flags must be an Integer".into(),
                    }))),
                };
                let mut st: i32 = 0;
                let r = unsafe { libc::waitpid(pid, &mut st, flags) };
                if r < 0 {
                    return Some(Err(self.trap(RubyError::HostException {
                        class_name: "Errno::ECHILD".to_string(),
                        message: format!("No child processes - waitpid({pid})"),
                    })));
                }
                if r == 0 {
                    // WNOHANG and the child hasn't changed state:
                    // CRuby's waitpid returns nil here.
                    return Some(Ok(Value::Nil));
                }
                let exitstatus: i64 = if libc::WIFEXITED(st) {
                    libc::WEXITSTATUS(st) as i64
                } else {
                    // Signalled child — surface 128+signo like a
                    // shell would; minitest only consumes the
                    // WIFEXITED path.
                    128 + libc::WTERMSIG(st) as i64
                };
                self.maybe_gc();
                if let Err(e) = self.check_alloc() { return Some(Err(e)); }
                let id = self.heap.alloc(HeapObj::Array(vec![
                    Value::Int(r as i64),
                    Value::Int(exitstatus),
                ].into()));
                Some(Ok(Value::Array(id)))
            }
            // ---- Real-fd pipe primitives (unix) --------------------
            // `IO.pipe`'s fd-backed half: the preamble wraps these raw
            // fds in RubyrsFdReader / RubyrsFdWriter so pipe endpoints
            // survive fork(2) — the parallel gem's work_in_processes
            // protocol (fork worker, Marshal frames over the pipe) is
            // the motivating consumer (rubocop --parallel). CLOEXEC is
            // set on both ends: fork keeps them (what we need), an
            // exec'd `Kernel#system` subprocess doesn't inherit them
            // (CRuby's pipes are CLOEXEC too).
            #[cfg(all(unix, not(target_os = "wasi")))]
            "__rubyrs_pipe" => {
                let mut fds = [0i32; 2];
                if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
                    let e = std::io::Error::last_os_error();
                    return Some(Err(self.trap(crate::vm::fileops::io_error(&e, None))));
                }
                unsafe {
                    libc::fcntl(fds[0], libc::F_SETFD, libc::FD_CLOEXEC);
                    libc::fcntl(fds[1], libc::F_SETFD, libc::FD_CLOEXEC);
                }
                self.maybe_gc();
                if let Err(e) = self.check_alloc() { return Some(Err(e)); }
                let id = self.heap.alloc(HeapObj::Array(vec![
                    Value::Int(fds[0] as i64),
                    Value::Int(fds[1] as i64),
                ].into()));
                Some(Ok(Value::Array(id)))
            }
            // `__rubyrs_fd_read(fd, len_or_nil)` — BLOCKING read(2).
            //   len = Integer n → read exactly n bytes (looping across
            //     short reads), returning fewer only at EOF; nil when
            //     EOF arrives before the first byte (IO#read(n)).
            //   len = nil → read to EOF; "" at immediate EOF (IO#read).
            // EINTR retries (a trapped SIGCHLD mustn't truncate a
            // Marshal frame mid-read).
            #[cfg(all(unix, not(target_os = "wasi")))]
            "__rubyrs_fd_read" => {
                let fd = match args.first() {
                    Some(Value::Int(n)) => *n as i32,
                    _ => return Some(Err(self.trap(RubyError::TypeError {
                        msg: "fd must be an Integer".into(),
                    }))),
                };
                let want: Option<usize> = match args.get(1) {
                    Some(Value::Int(n)) if *n >= 0 => Some(*n as usize),
                    Some(Value::Nil) | None => None,
                    _ => return Some(Err(self.trap(RubyError::TypeError {
                        msg: "read length must be a non-negative Integer or nil".into(),
                    }))),
                };
                let mut buf: Vec<u8> = Vec::new();
                let mut chunk = [0u8; 65536];
                loop {
                    let cap = match want {
                        Some(n) => {
                            let rem = n - buf.len();
                            if rem == 0 { break; }
                            rem.min(chunk.len())
                        }
                        None => chunk.len(),
                    };
                    let r = unsafe {
                        libc::read(fd, chunk.as_mut_ptr() as *mut libc::c_void, cap)
                    };
                    if r < 0 {
                        let e = std::io::Error::last_os_error();
                        if e.raw_os_error() == Some(libc::EINTR) { continue; }
                        return Some(Err(self.trap(crate::vm::fileops::io_error(&e, None))));
                    }
                    if r == 0 { break; } // EOF
                    buf.extend_from_slice(&chunk[..r as usize]);
                }
                match want {
                    Some(n) if n > 0 && buf.is_empty() => Some(Ok(Value::Nil)),
                    _ => Some(Ok(Value::new_str_bytes_binary(buf))),
                }
            }
            // `__rubyrs_fd_write(fd, str)` — full write(2) loop; EINTR
            // retries; a closed read end surfaces as Errno::EPIPE (the
            // parallel gem's DeadWorker discipline rescues it). Rust's
            // runtime already SIG_IGNs SIGPIPE, so write returns the
            // errno instead of killing the process.
            #[cfg(all(unix, not(target_os = "wasi")))]
            "__rubyrs_fd_write" => {
                let fd = match args.first() {
                    Some(Value::Int(n)) => *n as i32,
                    _ => return Some(Err(self.trap(RubyError::TypeError {
                        msg: "fd must be an Integer".into(),
                    }))),
                };
                let bytes: Vec<u8> = match args.get(1) {
                    Some(Value::Str(s)) => s.content.borrow().clone(),
                    _ => return Some(Err(self.trap(RubyError::TypeError {
                        msg: "write data must be a String".into(),
                    }))),
                };
                let mut off = 0usize;
                while off < bytes.len() {
                    let r = unsafe {
                        libc::write(
                            fd,
                            bytes[off..].as_ptr() as *const libc::c_void,
                            bytes.len() - off,
                        )
                    };
                    if r < 0 {
                        let e = std::io::Error::last_os_error();
                        if e.raw_os_error() == Some(libc::EINTR) { continue; }
                        return Some(Err(self.trap(crate::vm::fileops::io_error(&e, None))));
                    }
                    off += r as usize;
                }
                Some(Ok(Value::Int(bytes.len() as i64)))
            }
            #[cfg(all(unix, not(target_os = "wasi")))]
            "__rubyrs_fd_close" => {
                let fd = match args.first() {
                    Some(Value::Int(n)) => *n as i32,
                    _ => return Some(Err(self.trap(RubyError::TypeError {
                        msg: "fd must be an Integer".into(),
                    }))),
                };
                unsafe { libc::close(fd) };
                Some(Ok(Value::Nil))
            }
            // ---- Cooperative-scheduler fd primitives ----------------
            // The green-thread scheduler (preamble/thread.rb) turns
            // blocking pipe reads/writes into YIELD POINTS: a thread
            // that would block parks on the fd and the scheduler polls.
            // These are the non-blocking single-step halves; the
            // blocking `__rubyrs_fd_read`/`__rubyrs_fd_write` above stay
            // the zero-overhead single-threaded path.
            //
            // `__rubyrs_fd_read_step(fd, maxlen)` — ONE non-blocking
            // read attempt: `false` when the fd isn't readable yet
            // (caller parks), `nil` at EOF, else a 1..maxlen-byte
            // binary String. poll(2)-before-read instead of O_NONBLOCK
            // so the open file description's flags are never mutated
            // (they're shared with the forked child's copy).
            #[cfg(all(unix, not(target_os = "wasi")))]
            "__rubyrs_fd_read_step" => {
                let fd = match args.first() {
                    Some(Value::Int(n)) => *n as i32,
                    _ => return Some(Err(self.trap(RubyError::TypeError {
                        msg: "fd must be an Integer".into(),
                    }))),
                };
                let maxlen: usize = match args.get(1) {
                    Some(Value::Int(n)) if *n > 0 => (*n as usize).min(1 << 20),
                    _ => return Some(Err(self.trap(RubyError::TypeError {
                        msg: "read length must be a positive Integer".into(),
                    }))),
                };
                let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
                let pr = unsafe { libc::poll(&mut pfd, 1, 0) };
                if pr < 0 {
                    let e = std::io::Error::last_os_error();
                    if e.raw_os_error() == Some(libc::EINTR) {
                        return Some(Ok(Value::Bool(false)));
                    }
                    return Some(Err(self.trap(crate::vm::fileops::io_error(&e, None))));
                }
                if pr == 0 {
                    return Some(Ok(Value::Bool(false)));
                }
                let mut buf = vec![0u8; maxlen];
                let r = unsafe {
                    libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, maxlen)
                };
                if r < 0 {
                    let e = std::io::Error::last_os_error();
                    match e.raw_os_error() {
                        Some(libc::EINTR) | Some(libc::EAGAIN) => {
                            return Some(Ok(Value::Bool(false)));
                        }
                        _ => return Some(Err(self.trap(crate::vm::fileops::io_error(&e, None)))),
                    }
                }
                if r == 0 {
                    return Some(Ok(Value::Nil)); // EOF
                }
                buf.truncate(r as usize);
                Some(Ok(Value::new_str_bytes_binary(buf)))
            }
            // `__rubyrs_fd_write_step(fd, str, offset)` — ONE
            // non-blocking write attempt from byte `offset`: `false`
            // when the pipe buffer is full (caller parks), else the
            // byte count written. A closed read end surfaces as
            // Errno::EPIPE exactly like the blocking write.
            #[cfg(all(unix, not(target_os = "wasi")))]
            "__rubyrs_fd_write_step" => {
                let fd = match args.first() {
                    Some(Value::Int(n)) => *n as i32,
                    _ => return Some(Err(self.trap(RubyError::TypeError {
                        msg: "fd must be an Integer".into(),
                    }))),
                };
                let bytes: Vec<u8> = match args.get(1) {
                    Some(Value::Str(s)) => s.content.borrow().clone(),
                    _ => return Some(Err(self.trap(RubyError::TypeError {
                        msg: "write data must be a String".into(),
                    }))),
                };
                let off: usize = match args.get(2) {
                    Some(Value::Int(n)) if *n >= 0 => *n as usize,
                    None => 0,
                    _ => return Some(Err(self.trap(RubyError::TypeError {
                        msg: "write offset must be a non-negative Integer".into(),
                    }))),
                };
                if off >= bytes.len() {
                    return Some(Ok(Value::Int(0)));
                }
                let mut pfd = libc::pollfd { fd, events: libc::POLLOUT, revents: 0 };
                let pr = unsafe { libc::poll(&mut pfd, 1, 0) };
                if pr < 0 {
                    let e = std::io::Error::last_os_error();
                    if e.raw_os_error() == Some(libc::EINTR) {
                        return Some(Ok(Value::Bool(false)));
                    }
                    return Some(Err(self.trap(crate::vm::fileops::io_error(&e, None))));
                }
                if pr == 0 {
                    return Some(Ok(Value::Bool(false)));
                }
                let r = unsafe {
                    libc::write(
                        fd,
                        bytes[off..].as_ptr() as *const libc::c_void,
                        bytes.len() - off,
                    )
                };
                if r < 0 {
                    let e = std::io::Error::last_os_error();
                    match e.raw_os_error() {
                        Some(libc::EINTR) | Some(libc::EAGAIN) => {
                            return Some(Ok(Value::Bool(false)));
                        }
                        _ => return Some(Err(self.trap(crate::vm::fileops::io_error(&e, None)))),
                    }
                }
                Some(Ok(Value::Int(r as i64)))
            }
            // `__rubyrs_fd_poll(read_fds, write_fds, timeout_ms)` — the
            // scheduler's "nothing runnable" wait. Blocks in poll(2)
            // over every parked fd (timeout -1 = indefinitely, else
            // millisecond cap for sleeping threads) and returns
            // `[ready_read_fds, ready_write_fds]`. POLLHUP/POLLERR
            // count as ready (the woken reader then observes EOF /
            // EPIPE through its normal step call). EINTR returns two
            // empty arrays so the Ruby loop re-enters through the
            // dispatch safe-point (SIGINT traps stay deliverable).
            #[cfg(all(unix, not(target_os = "wasi")))]
            "__rubyrs_fd_poll" => {
                let read_fds: Vec<i32> = match args.first() {
                    Some(Value::Array(id)) => self
                        .heap
                        .array(*id)
                        .iter()
                        .filter_map(|v| match v {
                            Value::Int(n) => Some(*n as i32),
                            _ => None,
                        })
                        .collect(),
                    _ => return Some(Err(self.trap(RubyError::TypeError {
                        msg: "poll read set must be an Array of fds".into(),
                    }))),
                };
                let write_fds: Vec<i32> = match args.get(1) {
                    Some(Value::Array(id)) => self
                        .heap
                        .array(*id)
                        .iter()
                        .filter_map(|v| match v {
                            Value::Int(n) => Some(*n as i32),
                            _ => None,
                        })
                        .collect(),
                    Some(Value::Nil) | None => Vec::new(),
                    _ => return Some(Err(self.trap(RubyError::TypeError {
                        msg: "poll write set must be an Array of fds".into(),
                    }))),
                };
                let timeout_ms: i32 = match args.get(2) {
                    Some(Value::Int(n)) => (*n).clamp(-1, i32::MAX as i64) as i32,
                    Some(Value::Nil) | None => -1,
                    _ => return Some(Err(self.trap(RubyError::TypeError {
                        msg: "poll timeout must be an Integer (ms) or nil".into(),
                    }))),
                };
                let mut pfds: Vec<libc::pollfd> = read_fds
                    .iter()
                    .map(|&fd| libc::pollfd { fd, events: libc::POLLIN, revents: 0 })
                    .chain(write_fds.iter().map(|&fd| libc::pollfd {
                        fd,
                        events: libc::POLLOUT,
                        revents: 0,
                    }))
                    .collect();
                let (mut ready_r, mut ready_w) = (Vec::new(), Vec::new());
                let pr = unsafe {
                    libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, timeout_ms)
                };
                if pr < 0 {
                    let e = std::io::Error::last_os_error();
                    if e.raw_os_error() != Some(libc::EINTR) {
                        return Some(Err(self.trap(crate::vm::fileops::io_error(&e, None))));
                    }
                    // EINTR: fall through with empty ready sets.
                } else if pr > 0 {
                    for (i, pfd) in pfds.iter().enumerate() {
                        if pfd.revents == 0 {
                            continue;
                        }
                        if i < read_fds.len() {
                            ready_r.push(Value::Int(pfd.fd as i64));
                        } else {
                            ready_w.push(Value::Int(pfd.fd as i64));
                        }
                    }
                }
                self.maybe_gc();
                if let Err(e) = self.check_alloc() { return Some(Err(e)); }
                let rid = self.heap.alloc(HeapObj::Array(ready_r.into()));
                // Pin the first result array across the following
                // allocations (GC rooting discipline — pin before
                // alloc; see the recurring bug-class note).
                let mut g = crate::vm::PinGuard::new(self);
                g.pin(Value::Array(rid));
                let wid = g.vm.heap.alloc(HeapObj::Array(ready_w.into()));
                g.pin(Value::Array(wid));
                let outer = g.vm.heap.alloc(HeapObj::Array(vec![
                    Value::Array(rid),
                    Value::Array(wid),
                ].into()));
                drop(g);
                Some(Ok(Value::Array(outer)))
            }
            // `__rubyrs_nprocessors` — honest logical-core count for
            // Etc.nprocessors. With the cooperative scheduler giving
            // `rubocop --parallel` real N-way supervision, the parallel
            // gem's `processor_count` should size worker pools to the
            // machine, exactly like CRuby.
            "__rubyrs_nprocessors" => {
                let n = std::thread::available_parallelism()
                    .map(|n| n.get() as i64)
                    .unwrap_or(1);
                Some(Ok(Value::Int(n)))
            }
            "system" => {
                if !self.allow_process_spawn {
                    return Some(Ok(Value::Nil));
                }
                let mut argv: Vec<String> = Vec::with_capacity(args.len());
                for a in args {
                    match a {
                        Value::Str(s) => argv.push(s.to_string_lossy()),
                        other => argv.push(other.to_display(&self.heap, &self.interner)),
                    }
                }
                if argv.is_empty() {
                    return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: "wrong number of arguments (given 0, expected 1+)".into(),
                    })));
                }
                // CRuby's single-string form only routes through
                // the shell when shell metacharacters are present;
                // a bare word execs directly, so a missing command
                // is a clean ENOENT → nil (no shell noise) — the
                // probe shape minitest's diff discovery relies on.
                let needs_shell = argv.len() == 1
                    && argv[0].bytes().any(|b| {
                        matches!(b, b' ' | b'\t' | b'*' | b'?' | b'{' | b'}' | b'[' | b']'
                            | b'<' | b'>' | b'|' | b'&' | b';' | b'(' | b')' | b'$'
                            | b'`' | b'\\' | b'"' | b'\'' | b'~' | b'#' | b'\n')
                    });
                let mut cmd = if argv.len() == 1 && needs_shell {
                    let mut c = std::process::Command::new("/bin/sh");
                    c.arg("-c").arg(&argv[0]);
                    c
                } else if argv.len() == 1 {
                    std::process::Command::new(&argv[0])
                } else {
                    let mut c = std::process::Command::new(&argv[0]);
                    c.args(&argv[1..]);
                    c
                };
                // capture_subprocess_io: when $stdout/$stderr are
                // reopen-delegating veneers (Tempfile-backed — no
                // real fd to hand the child), capture the child's
                // pipes and forward the bytes through the veneer's
                // Ruby-level write, the same path Kernel#puts takes
                // under redirection.
                let out_redirect = self.stdio_redirect("$stdout", true);
                let err_redirect = self.stdio_redirect("$stderr", true);
                if out_redirect.is_none() && err_redirect.is_none() {
                    return Some(Ok(match cmd.status() {
                        Ok(st) => Value::Bool(st.success()),
                        Err(_) => Value::Nil,
                    }));
                }
                let out = match cmd.output() {
                    Ok(o) => o,
                    Err(_) => return Some(Ok(Value::Nil)),
                };
                if !out.stdout.is_empty() {
                    let s = String::from_utf8_lossy(&out.stdout).into_owned();
                    match out_redirect {
                        Some(t) => {
                            if let Err(e) = self.forward_stdio_call(t, "write", &[Value::new_str(s)]) {
                                return Some(Err(e));
                            }
                        }
                        None => print!("{s}"),
                    }
                }
                if !out.stderr.is_empty() {
                    let s = String::from_utf8_lossy(&out.stderr).into_owned();
                    match err_redirect {
                        Some(t) => {
                            if let Err(e) = self.forward_stdio_call(t, "write", &[Value::new_str(s)]) {
                                return Some(Err(e));
                            }
                        }
                        None => eprint!("{s}"),
                    }
                }
                Some(Ok(Value::Bool(out.status.success())))
            }
            // Backtick / `%x{}` — capture stdout as a String. Off →
            // the same catchable RuntimeError the old compile-time
            // lowering raised (safe_yaml's `(\`which dpkg\` rescue
            // '')` probe shape depends on rescuability).
            "__rubyrs_backtick" => {
                if !self.allow_process_spawn {
                    return Some(Err(self.trap(RubyError::RuntimeError {
                        msg: "rubyrs: backtick / %x command execution is not available (process spawn capability is off)".into(),
                    })));
                }
                let cmd = match args.first() {
                    Some(Value::Str(s)) => s.to_string_lossy(),
                    _ => return Some(Err(self.trap(RubyError::TypeError {
                        msg: "backtick command must be a String".into(),
                    }))),
                };
                // Same simple-command split as Kernel#system: a
                // bare word execs directly so a missing command is
                // CRuby's Errno::ENOENT raise (the
                // `(\`which dpkg\` rescue '')` probe shape);
                // metacharacters route through the shell, whose
                // missing-command behavior (empty stdout, message
                // on stderr) also matches CRuby.
                let needs_shell = cmd.bytes().any(|b| {
                    matches!(b, b' ' | b'\t' | b'*' | b'?' | b'{' | b'}' | b'[' | b']'
                        | b'<' | b'>' | b'|' | b'&' | b';' | b'(' | b')' | b'$'
                        | b'`' | b'\\' | b'"' | b'\'' | b'~' | b'#' | b'\n')
                });
                let output = if needs_shell {
                    std::process::Command::new("/bin/sh")
                        .arg("-c")
                        .arg(&cmd)
                        .output()
                } else {
                    std::process::Command::new(&cmd).output()
                };
                match output {
                    Ok(out) => Some(Ok(Value::new_str(
                        String::from_utf8_lossy(&out.stdout).into_owned(),
                    ))),
                    Err(_) => Some(Err(self.trap(RubyError::HostException {
                        class_name: "Errno::ENOENT".to_string(),
                        message: format!("No such file or directory - {cmd}"),
                    }))),
                }
            }
            "__rubyrs_math" => {
                fn as_f64(v: &Value) -> Option<f64> {
                    match v {
                        Value::Int(n) => Some(*n as f64),
                        Value::Float(f) => Some(*f),
                        Value::Rational(_) => None, // preamble converts first
                        _ => None,
                    }
                }
                let op = match args.first() {
                    Some(Value::Sym(s)) => self.interner.resolve(*s).clone(),
                    _ => return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: "__rubyrs_math: op symbol required".into(),
                    }))),
                };
                let x = match args.get(1).and_then(as_f64) {
                    Some(v) => v,
                    None => return Some(Err(self.trap(RubyError::TypeError {
                        msg: "can't convert into Float".into(),
                    }))),
                };
                let y = args.get(2).and_then(as_f64);
                let r = match (&*op, y) {
                    ("sqrt", _) => x.sqrt(),
                    ("cbrt", _) => x.cbrt(),
                    ("exp", _) => x.exp(),
                    ("log", None) => x.ln(),
                    ("log", Some(base)) => x.log(base),
                    ("log2", _) => x.log2(),
                    ("log10", _) => x.log10(),
                    ("sin", _) => x.sin(),
                    ("cos", _) => x.cos(),
                    ("tan", _) => x.tan(),
                    ("asin", _) => x.asin(),
                    ("acos", _) => x.acos(),
                    ("atan", _) => x.atan(),
                    ("atan2", Some(b)) => x.atan2(b),
                    ("sinh", _) => x.sinh(),
                    ("cosh", _) => x.cosh(),
                    ("tanh", _) => x.tanh(),
                    ("hypot", Some(b)) => x.hypot(b),
                    ("pow", Some(b)) => x.powf(b),
                    _ => return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: format!("__rubyrs_math: unknown op {op}"),
                    }))),
                };
                Some(Ok(Value::Float(r)))
            }
            // `__rubyrs_marshal_load_binary(bytes)` — load-only reader
            // for CRuby's Marshal 4.8 byte format, COMMON-TAG subset:
            // nil/true/false, Fixnum, Float, String (raw + I-wrapped
            // encoding ivars), Symbol (+ symlink), Array, Hash, and
            // object links. Anything else (Bignum, user classes,
            // Time, Struct, regexp, extended/user-marshal forms)
            // raises TypeError naming the tag — fail-loud, the same
            // contract the token loader keeps. Motivating consumer:
            // addressable's pregenerated unicode.data table
            // (Hash{Int => Array[Int|nil]}, 4233 keys).
            // `Encoding.default_external=` host half: resolve the
            // name and stamp Vm::default_external (what tag-less
            // File.read uses). Unknown name → CRuby's ArgumentError.
            // `Encoding.default_internal=` host half. nil clears
            // (CRuby's default); a name resolves strictly.
            "__rubyrs_set_default_internal" => {
                match args.first() {
                    Some(Value::Nil) | None => {
                        self.default_internal = None;
                        Some(Ok(Value::Nil))
                    }
                    Some(Value::Str(s)) => {
                        let name = s.to_string_lossy();
                        match Self::encoding_tag_from_str(&name) {
                            Some(tag) => {
                                self.default_internal = Some(tag);
                                Some(Ok(Value::Nil))
                            }
                            None => Some(Err(self.trap(RubyError::ArgumentError {
                                msg: format!("unknown encoding name - {name}"),
                            }))),
                        }
                    }
                    _ => Some(Err(self.trap(RubyError::TypeError {
                        msg: "encoding name must be a String or nil".into(),
                    }))),
                }
            }
            "__rubyrs_set_default_external" => {
                let name = match args.first() {
                    Some(Value::Str(s)) => s.to_string_lossy(),
                    _ => return Some(Err(self.trap(RubyError::TypeError {
                        msg: "encoding name must be a String".into(),
                    }))),
                };
                match Self::encoding_tag_from_str(&name) {
                    Some(tag) => {
                        self.default_external = tag;
                        Some(Ok(Value::Nil))
                    }
                    None => Some(Err(self.trap(RubyError::ArgumentError {
                        msg: format!("unknown encoding name - {name}"),
                    }))),
                }
            }
            // Unicode normalization / grapheme segmentation for
            // non-ASCII input — backed by the `unicode-normalization` /
            // `unicode-segmentation` crates behind `_encoding_full`. The
            // preamble (`string_ext.rb`) handles the ASCII fast path and
            // form validation, and only calls these for non-ASCII; absent
            // the feature they raise NotImplementedError (the previous
            // "non-ASCII not supported" surface, unchanged).
            "__rubyrs_unicode_normalize" => {
                let Some(Value::Str(s)) = args.first() else {
                    return Some(Ok(Value::Nil));
                };
                let text = s.to_string_lossy();
                let form = match args.get(1) {
                    Some(Value::Sym(f)) => self.interner.resolve(*f).to_string(),
                    _ => "nfc".to_string(),
                };
                #[cfg(feature = "_encoding_full")]
                {
                    use unicode_normalization::UnicodeNormalization;
                    let out: String = match form.as_str() {
                        "nfc" => text.nfc().collect(),
                        "nfd" => text.nfd().collect(),
                        "nfkc" => text.nfkc().collect(),
                        "nfkd" => text.nfkd().collect(),
                        _ => return Some(Err(self.trap(RubyError::ArgumentError {
                            msg: format!("invalid normalization form {form}"),
                        }))),
                    };
                    return Some(Ok(Value::new_str(out)));
                }
                #[cfg(not(feature = "_encoding_full"))]
                {
                    let _ = (text, form);
                    Some(Err(self.trap(RubyError::HostException {
                        class_name: "NotImplementedError".to_string(),
                        message: "String#unicode_normalize: non-ASCII input is not supported in this build (enable _encoding_full)".to_string(),
                    })))
                }
            }
            "__rubyrs_unicode_normalized_p" => {
                let Some(Value::Str(s)) = args.first() else {
                    return Some(Ok(Value::Bool(true)));
                };
                let text = s.to_string_lossy();
                let form = match args.get(1) {
                    Some(Value::Sym(f)) => self.interner.resolve(*f).to_string(),
                    _ => "nfc".to_string(),
                };
                #[cfg(feature = "_encoding_full")]
                {
                    use unicode_normalization::UnicodeNormalization;
                    let norm: String = match form.as_str() {
                        "nfc" => text.nfc().collect(),
                        "nfd" => text.nfd().collect(),
                        "nfkc" => text.nfkc().collect(),
                        "nfkd" => text.nfkd().collect(),
                        _ => return Some(Err(self.trap(RubyError::ArgumentError {
                            msg: format!("invalid normalization form {form}"),
                        }))),
                    };
                    return Some(Ok(Value::Bool(norm == text)));
                }
                #[cfg(not(feature = "_encoding_full"))]
                {
                    let _ = (text, form);
                    Some(Err(self.trap(RubyError::HostException {
                        class_name: "NotImplementedError".to_string(),
                        message: "String#unicode_normalized?: non-ASCII input is not supported in this build (enable _encoding_full)".to_string(),
                    })))
                }
            }
            "__rubyrs_grapheme_split" => {
                let Some(Value::Str(s)) = args.first() else {
                    return Some(Ok(Value::Nil));
                };
                let text = s.to_string_lossy();
                #[cfg(feature = "_encoding_full")]
                {
                    use unicode_segmentation::UnicodeSegmentation;
                    let parts: Vec<Value> = text
                        .graphemes(true)
                        .map(|g| Value::new_str(g.to_string()))
                        .collect();
                    self.maybe_gc();
                    if let Err(t) = self.check_alloc() {
                        return Some(Err(t));
                    }
                    let id = self.heap.alloc(HeapObj::Array(parts.into()));
                    return Some(Ok(Value::Array(id)));
                }
                #[cfg(not(feature = "_encoding_full"))]
                {
                    let _ = text;
                    Some(Err(self.trap(RubyError::HostException {
                        class_name: "NotImplementedError".to_string(),
                        message: "String#each_grapheme_cluster: non-ASCII input is not supported in this build (enable _encoding_full)".to_string(),
                    })))
                }
            }
            "__rubyrs_marshal_load_binary" => {
                let bytes: Vec<u8> = match args.first() {
                    Some(Value::Str(s)) => s.content.borrow().clone(),
                    _ => return Some(Err(self.trap(RubyError::TypeError {
                        msg: "marshal data must be a String".into(),
                    }))),
                };
                if bytes.len() < 2 || bytes[0] != 0x04 || bytes[1] != 0x08 {
                    return Some(Err(self.trap(RubyError::TypeError {
                        msg: "incompatible marshal file format (can't be read)".into(),
                    })));
                }
                let mut rd = MarshalReader {
                    b: &bytes,
                    pos: 2,
                    symbols: Vec::new(),
                    objects: Vec::new(),
                    trap: None,
                    next_hash_by_identity: false,
                };
                // Every container the reader allocates stays pinned
                // until the WHOLE graph is wired (children are only
                // reachable from Rust locals until their parent's
                // final write-back) — one truncate releases them
                // all, success or error.
                let pin_base = self.pinned.len();
                let r = rd.read_value(self);
                self.pinned.truncate(pin_base);
                match r {
                    Ok(v) => Some(Ok(v)),
                    // A reentrant `_load` / `marshal_load` hook that raised
                    // propagates its own exception; everything else is a
                    // malformed-stream TypeError.
                    Err(msg) => match rd.trap {
                        Some(t) => Some(Err(t)),
                        None => Some(Err(self.trap(RubyError::TypeError { msg }))),
                    },
                }
            }
            // `__rubyrs_marshal_dump_binary(obj)` → a real CRuby-4.8
            // byte stream for the common-tag subset, or nil when the
            // graph contains anything outside it (the caller then uses
            // the same-process registry token). A successful dump is
            // byte-loadable by both MarshalReader and real CRuby, so
            // `Marshal.load(Marshal.dump(x))` deep-copies these types.
            "__rubyrs_marshal_dump_binary" => {
                let obj = args.first().cloned().unwrap_or(Value::Nil);
                // Gate on the dumpability probe FIRST: an un-dumpable
                // graph (Proc/Method/Binding/IO, a singleton-augmented or
                // genuinely-anonymous object) returns nil here so
                // `Marshal.dump` falls through to the registry token,
                // whose own probe raises CRuby's TypeError. This also
                // guarantees the writer only ever sees Instance-backed
                // objects (no panic on an opaque host shape).
                if self.marshal_dumpable(&obj).is_err() {
                    return Some(Ok(Value::Nil));
                }
                let e_sym = self.interner.intern("E");
                let mut w = MarshalWriter {
                    out: vec![0x04, 0x08],
                    symbols: Vec::new(),
                    objects: Vec::new(),
                    e_sym,
                    trap: None,
                };
                // Pin the whole graph: reentrant `_dump` / `marshal_dump`
                // hooks run Ruby (which may GC), and the Rust-local `obj`
                // isn't itself a root — pinning it keeps the reachable
                // subgraph alive across the hook.
                let pin_base = self.pinned.len();
                self.pinned.push(obj.clone());
                let res = w.write_value(self, &obj);
                self.pinned.truncate(pin_base);
                match res {
                    Ok(()) => Some(Ok(Value::new_str_bytes_binary(w.out))),
                    // A hook raised → propagate the user exception instead
                    // of falling back to the token (which would swallow
                    // it). Out-of-subset (no trap) → token fallback.
                    Err(()) => match w.trap {
                        Some(t) => Some(Err(t)),
                        None => Some(Ok(Value::Nil)),
                    },
                }
            }
            // `__rubyrs_marshal_check_dumpable(obj)` — the dumpability
            // probe as a standalone raise-or-nil. The IO-framed dump
            // path uses it to produce CRuby's TypeError ("no _dump_data
            // is defined for class Proc", "singleton can't be dumped",
            // ...) BEFORE declaring an in-subset-but-unbyteable graph a
            // rubyrs limitation — a registry token can't cross the
            // process boundary an IO port implies, so there is no token
            // fallback on that path.
            "__rubyrs_marshal_check_dumpable" => {
                let obj = args.first().cloned().unwrap_or(Value::Nil);
                match self.marshal_dumpable(&obj) {
                    Ok(()) => Some(Ok(Value::Nil)),
                    Err(why) => Some(Err(self.trap(RubyError::TypeError { msg: why }))),
                }
            }
            "__rubyrs_marshal_stash" => {
                const MARSHAL_REGISTRY_CAP: usize = 1024;
                let obj = args.first().cloned().unwrap_or(Value::Nil);
                // CRuby's Marshal.dump REJECTS shapes that can't be
                // byte-serialized; real callers rely on the raise as
                // a dumpability PROBE (minitest's sanitize_exception
                // routes un-dumpable exceptions into its neuter
                // chain). Mirror the common rejections — procs and
                // friends, IO-ish typed data, anonymous classes,
                // singleton-augmented objects — by walking the value
                // graph (cycle-safe, ivars + container elements).
                if let Err(why) = self.marshal_dumpable(&obj) {
                    return Some(Err(self.trap(RubyError::TypeError { msg: why })));
                }
                if self.marshal_registry.len() >= MARSHAL_REGISTRY_CAP {
                    // Cap reached: degrade to the plain placeholder
                    // (no token) — load of it raises TypeError, the
                    // honest "can't round-trip" answer.
                    return Some(Ok(Value::new_str("--- {}\n".to_string())));
                }
                let idx = self.marshal_registry.len();
                self.marshal_registry.push(obj);
                Some(Ok(Value::new_str(format!("--- {{}}\n# rubyrs-marshal:{idx}\n"))))
            }
            // `__rubyrs_marshal_fetch(str)` → one-element Array
            // [obj] on a token hit, nil otherwise (the wrapper
            // distinguishes a legitimately-dumped nil from a miss).
            "__rubyrs_marshal_fetch" => {
                let token = match args.first() {
                    Some(Value::Str(s)) => s.to_string_lossy(),
                    _ => return Some(Ok(Value::Nil)),
                };
                let idx: Option<usize> = token
                    .strip_prefix("--- {}\n# rubyrs-marshal:")
                    .and_then(|rest| rest.strip_suffix('\n'))
                    .and_then(|n| n.parse().ok());
                match idx.and_then(|i| self.marshal_registry.get(i).cloned()) {
                    Some(v) => {
                        let mut g = PinGuard::new(self);
                        g.pin(v.clone());
                        g.vm.maybe_gc();
                        if let Err(t) = g.vm.check_alloc() {
                            return Some(Err(t));
                        }
                        let id = g.vm.heap.alloc(HeapObj::Array(vec![v].into()));
                        Some(Ok(Value::Array(id)))
                    }
                    None => Some(Ok(Value::Nil)),
                }
            }
            "at_exit" => {
                Some(Err(self.trap(RubyError::LocalJumpError {
                    msg: "no block given (at_exit)".into(),
                })))
            }
            "__rubyrs_signal_trap" => {
                if args.len() != 2 {
                    return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: format!(
                            "__rubyrs_signal_trap(sig, handler) — expected 2 args, got {}",
                            args.len(),
                        ),
                    })));
                }
                let sig = match crate::signals::parse_signal_name(&args[0], &self.interner) {
                    Some(n) => n,
                    None => return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: format!(
                            "unsupported signal {:?}", args[0].to_display(&self.heap, &self.interner),
                        ),
                    }))),
                };
                // v7 round-3: reject SIGKILL (9) and SIGSTOP (19).
                // CRuby raises ArgumentError for these because the
                // kernel forbids userspace trapping of them. The
                // numbers are POSIX-standard; signal-hook would
                // also reject them at install time, but raising
                // here gives a clear ArgumentError before the
                // ServeOptions/trap flow gets confusing.
                if sig == 9 || sig == 19 {
                    let name = if sig == 9 { "KILL" } else { "STOP" };
                    return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: format!("can't trap reserved signal: SIG{name}"),
                    })));
                }
                // v7 round-3: the preamble's 1-arg-no-block form
                // sends a sentinel Symbol so the host fn can
                // distinguish QUERY mode from explicit
                // `Signal.trap(sig, nil)` (the latter is CRuby's
                // IGNORE shorthand).
                let query_sentinel = self.interner.intern("__rubyrs_query_mode__");
                let is_query = matches!(&args[1], Value::Sym(s) if *s == query_sentinel);
                let handler = &args[1];
                // Parse the new state. Query sentinel → no install;
                // explicit nil → IGNORE (CRuby parity).
                let new_state: Option<crate::vm::SignalHandlerState> = if is_query {
                    None
                } else if matches!(handler, Value::Nil) {
                    // Explicit nil = IGNORE per CRuby 3.x.
                    Some(crate::vm::SignalHandlerState::Ignore)
                } else { match handler {
                    Value::Nil => unreachable!(),
                    Value::Str(s) => {
                        let raw = s.to_string_lossy();
                        let normalized = raw.strip_prefix("SIG_").unwrap_or(&raw);
                        match normalized {
                            "DEFAULT" => Some(crate::vm::SignalHandlerState::Default),
                            "IGNORE" | "IGN" => Some(crate::vm::SignalHandlerState::Ignore),
                            _ => return Some(Err(self.trap(RubyError::ArgumentError {
                                msg: format!("unrecognized command \"{raw}\" for Signal.trap"),
                            }))),
                        }
                    }
                    Value::Sym(id) => {
                        let raw = self.interner.resolve(*id).clone();
                        let normalized = raw.strip_prefix("SIG_").unwrap_or(&raw);
                        match normalized {
                            "DEFAULT" => Some(crate::vm::SignalHandlerState::Default),
                            "IGNORE" | "IGN" => Some(crate::vm::SignalHandlerState::Ignore),
                            _ => return Some(Err(self.trap(RubyError::ArgumentError {
                                msg: format!("unrecognized command :{raw} for Signal.trap"),
                            }))),
                        }
                    }
                    Value::Block(id) => Some(crate::vm::SignalHandlerState::Block(*id)),
                    other => return Some(Err(self.trap(RubyError::TypeError {
                        msg: format!(
                            "trap handler must be \"DEFAULT\" / \"IGNORE\" / Proc / block, got {}",
                            other.type_name(),
                        ),
                    }))),
                } };
                // Read previous (default to Default if none).
                let previous = self.signal_traps.get(&sig)
                    .cloned()
                    .unwrap_or(crate::vm::SignalHandlerState::Default);
                if let Some(new) = new_state {
                    self.signal_traps.insert(sig, new);
                }
                // Convert previous state back to Ruby value.
                let ret = signal_handler_state_to_value(self, previous);
                Some(Ok(ret))
            }
            "sprintf" | "format" => {
                if args.is_empty() {
                    // CRuby's exact message — verified against MRI
                    // 3.x for `sprintf` with no args.
                    return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: "too few arguments".into(),
                    })));
                }
                let fmt = match &args[0] {
                    Value::Str(s) => s.to_string_lossy(),
                    other => {
                        return Some(Err(self.trap(RubyError::TypeError {
                            msg: format!(
                                "no implicit conversion of {} into String",
                                other.conv_type_name(),
                            ),
                        })));
                    }
                };
                let (fmt_args, p_overrides) = match self.sprintf_prepare_args(&fmt, &args[1..]) {
                    Ok(v) => v,
                    Err(t) => return Some(Err(t)),
                };
                let out = match crate::vm::ruby_sprintf(
                    &fmt, &fmt_args, &self.heap, &self.interner, self.max_value_bytes,
                    &p_overrides,
                ) {
                    Ok(s) => s,
                    Err(e) => return Some(Err(self.trap(e))),
                };
                if let Some(max) = self.max_value_bytes
                    && out.len() > max
                {
                    return Some(Err(self.trap(RubyError::ResourceExhausted {
                        msg: format!("sprintf would exceed {} bytes", max),
                    })));
                }
                Some(Ok(Value::new_str(out)))
            }
            // `require` — supports two file kinds:
            //
            //   - Ruby source (`.rb` extension, or no extension with
            //     a `.rb` sibling present): parse + compile + execute
            //     the file in this Vm (same machinery as
            //     `require_relative`, but resolved relative to cwd
            //     instead of caller's directory). Lets gem `lib/`
            //     wrapper files (msgpack's `register_type` /
            //     `MessagePack::Bigint`, etc.) load without the
            //     caller hand-rolling them.
            //
            //   - C extension (`.dylib` / `.bundle` / `.so` /
            //     `.dll`, or no extension with such a sibling
            //     present): dlopen + `Init_<stem>`. Existing
            //     `cext_require` path.
            //
            // Detection rule: if the resolved file ends in `.rb`,
            // route to the Ruby loader; otherwise fall through to
            // the cext loader. The Ruby loader also auto-appends
            // `.rb` when the input has no extension, so plain
            // `require "foo"` finds `foo.rb` first if it exists,
            // and only falls through to the cext path when the
            // `.rb` lookup fails.
            //
            // `$LOAD_PATH` walking is in place
            // (see `ruby_source_candidates`); `load` and
            // `autoload` are still deferred. Auto-populated
            // stdlib/gem paths are NOT pre-seeded — scripts
            // opt in by mutating `$LOAD_PATH` themselves.
            "require" => {
                // CRuby's `require` accepts anything convertible to a path
                // (Pathname etc.) via `to_path`/`to_str` (rb_get_path). Coerce
                // a single non-String arg here so `require Pathname.new(...)`
                // works; zeitwerk's loaders pass abspaths that may be Pathnames.
                let coerced: Option<Value> = match args {
                    [Value::Str(_)] => None,
                    [v] => match self.coerce_path_string(v, &[]) {
                        Ok(o) => o.map(Value::new_str),
                        Err(t) => return Some(Err(t)),
                    },
                    _ => None,
                };
                let args: &[Value] = match &coerced {
                    Some(v) => std::slice::from_ref(v),
                    None => args,
                };
                match args {
                [Value::Str(path)] => {
                    // ADR 0036: rubyrs can't dlopen the prism C extension (a CRuby-ABI
                    // `.bundle`), but the prism C library is linked in + exposed via the
                    // `__rubyrs_prism_serialize_parse*` host fns. When the prism gem's
                    // prism.rb requires "prism/prism", inject rubyrs's pure-Ruby backend
                    // (the gem supplies node/parse_result/serialize; we supply native parse)
                    // so `require "prism"` works + RuboCop's parser_prism engine runs.
                    // require-once via `loaded_stdlib_stubs`.
                    //
                    // `_prism_native` gate: the whole Rust bridge (prism_native +
                    // prism_wq + commdrv) is feature-gated out of the default
                    // build (~380 KB .text clawback); both injections go with it.
                    // The feature-off arm below keeps the failure loud.
                    //
                    // wasi gate: on wasm32-wasi `require` always raises
                    // LoadError (see the `target_os = "wasi"` arm below),
                    // so neither injection can fire — and the machinery
                    // they lean on (`loaded_stdlib_stubs`,
                    // `compile_and_run_source`) is cfg'd out of the Vm on
                    // wasi as dead code. Gate the whole block to match.
                    #[cfg(all(feature = "_prism_native", not(target_os = "wasi")))]
                    {
                        let p = path.to_string_lossy();
                        if &*p == "prism/prism" {
                            if self.loaded_stdlib_stubs.contains(&*p) {
                                return Some(Ok(Value::Bool(false)));
                            }
                            self.loaded_stdlib_stubs.insert(p.to_string());
                            if let Err(t) = self.compile_and_run_source(
                                std::path::PathBuf::from("<rubyrs:prism>"),
                                crate::prism_native::BACKEND_RB.to_string(),
                            ) {
                                return Some(Err(t));
                            }
                            return Some(Ok(Value::Bool(true)));
                        }
                        // prism_wq: after the gem's translation/parser.rb loads
                        // (usually via the Translation::Parser33/34 autoload),
                        // layer the native-tokenize hook over it — the
                        // native-first-with-per-file-fallback seam for
                        // RuboCop's prism engine. Inject-once. (wasi-
                        // gated by the enclosing block.)
                        if &*p == "prism/translation/parser"
                            && !self.loaded_stdlib_stubs.contains("<rubyrs:wqtrans-hook>")
                            && self.allow_filesystem_io
                            && self.find_ruby_source_candidate(&p)
                        {
                            return Some(match self.require_ruby(&p) {
                                Ok(v) => {
                                    self.loaded_stdlib_stubs
                                        .insert("<rubyrs:wqtrans-hook>".to_string());
                                    match self.compile_and_run_source(
                                        std::path::PathBuf::from("<rubyrs:wqtrans>"),
                                        crate::prism_wq::HOOK_RB.to_string(),
                                    ) {
                                        Ok(_) => Ok(v),
                                        Err(t) => Err(t),
                                    }
                                }
                                Err(t) => Err(t),
                            });
                        }
                    }
                    // `_prism_native` OFF: the prism gem's own
                    // `require "prism/prism"` (its C-extension load, which the
                    // injection above satisfies when the feature is built in)
                    // cannot be satisfied — the gem has no pure-Ruby parser, so
                    // there is no honest slow path to degrade to. Fail loudly
                    // with a feature-absent LoadError naming the rebuild flag
                    // (sass/regex-off precedent) instead of letting the request
                    // fall through to the cext dlopen path's cryptic miss.
                    #[cfg(not(feature = "_prism_native"))]
                    {
                        if &*path.to_string_lossy() == "prism/prism" {
                            return Some(Err(self.trap(RubyError::LoadError {
                                msg: "cannot load such file -- prism/prism \
                                      (rubyrs's native prism backend is not built into \
                                      this binary; rebuild with `--features _prism_native` \
                                      to run the prism gem / RuboCop's parser_prism engine)"
                                    .to_string(),
                            })));
                        }
                    }
                    #[cfg(not(target_os = "wasi"))]
                    {
                        let path_str = path.to_string_lossy();
                        // Probe for a `.rb` candidate first, regardless
                        // of cfg!("cext"). Walks the same candidate
                        // list `require_ruby` consults — as-given +
                        // each `$LOAD_PATH` entry + `name.rb` + raw-
                        // input fallback. Pre-pass-10-layer-#6 this
                        // also included the caller source file's
                        // directory and its parent (the cross-package
                        // "lib" hop), but those shadowed the stdlib-
                        // stub fallback when a `require` inside an
                        // already-loaded file resolved back to that
                        // same file. Co-located trees opt in by
                        // `$LOAD_PATH.unshift(__dir__)` (see the
                        // require_xpkg fixture's loader.rb). The
                        // routing here only DECIDES .rb vs cext —
                        // `find_ruby_source_candidate` runs the
                        // same probe `require_ruby` will run.
                        //
                        // Under the FS sandbox (`Config::allow_filesystem_io:
                        // false`), skip the probe — it'd touch the host FS
                        // before any Ruby-level resolution decides whether
                        // the load is in-process (stub / constant-satisfied)
                        // or actually wants disk. The downstream `cext_require`
                        // fallback gates separately; the stub / satisfied
                        // branches run unblocked because they don't touch FS.
                        //
                        // Scope pre-emption: when an allowlist is configured
                        // and the script supplied an ABSOLUTE path that
                        // lies outside every prefix, trap LoadError with the
                        // scope-gate message immediately. Without this, an
                        // out-of-scope absolute path would route to the cext
                        // fallback when find_ruby_source_candidate skipped
                        // the existence probe (closing the stat side-channel),
                        // and the cext fallback's generic
                        // `LoadError: cannot load such file -- <path>` would
                        // overwrite the more revealing scope-gate diagnostic.
                        // (Pre-LoadError this comment described the wrong
                        // exception class — both branches now raise LoadError;
                        // the pre-emption is about message clarity, not class.)
                        let scope_violation: Option<std::path::PathBuf> = if self
                            .allow_filesystem_io
                            && let Some(prefixes) = self.allowed_paths.as_ref()
                            && std::path::Path::new(&*path_str).is_absolute()
                        {
                            let resolved = crate::lexically_resolve_path(
                                std::path::Path::new(&*path_str),
                            );
                            if prefixes.iter().any(|pfx| resolved.starts_with(pfx)) {
                                None
                            } else {
                                Some(resolved)
                            }
                        } else {
                            None
                        };
                        if let Some(resolved) = scope_violation {
                            return Some(Err(self.trap(RubyError::LoadError {
                                msg: format!(
                                    "require blocked: path {:?} outside Config::allowed_paths",
                                    resolved,
                                ),
                            })));
                        }
                        // Blessed-reimpl override (ADR 0026): for a few
                        // names rubyrs ships a vendored implementation
                        // that MUST win over any on-disk gem of the
                        // same name, because the real gem can't run on
                        // rubyrs (e.g. safe_yaml subclasses
                        // Psych::Handler). Skip the LOAD_PATH probe so
                        // the require routes to the stub/vendor path
                        // below even when the gem is installed.
                        let force_vendor = is_blessed_reimpl_name(&path_str);
                        let rb_found = !force_vendor
                            && self.allow_filesystem_io
                            && self.find_ruby_source_candidate(&path_str);
                        if rb_found {
                            Some(self.require_ruby(&path_str))
                        } else if is_stdlib_stub_name(&path_str) {
                            // Tier 1 lenient stub for known pure-
                            // Ruby stdlib names. Returns `true` on
                            // first load and `false` on every
                            // subsequent require (CRuby's
                            // loaded-features dedup semantics).
                            // The actual stdlib code isn't
                            // loaded; scripts that only
                            // `require 'uri'` for feature
                            // detection proceed past the require
                            // line. Use of the stubbed-out
                            // stdlib fails later with a more
                            // specific NameError / NoMethodError,
                            // which is the right surface for
                            // "feature absent" vs "load failed".
                            // See ADR 0017 — stdlib is Tier 3;
                            // this is the embeddable-host
                            // lenient-mode bridge.
                            let already_loaded = self.loaded_stdlib_stubs.contains(&*path_str);
                            if already_loaded {
                                return Some(Ok(Value::Bool(false)));
                            }
                            self.loaded_stdlib_stubs.insert(path_str.to_string());
                            // Under `--features stdlib`, run the
                            // embedded pure-Ruby implementation (if
                            // any) on the current Vm. The default
                            // build's lenient stub path stops at the
                            // constant-shell loop below, preserving
                            // ADR 0017's "feature-absent surface"
                            // for stdlib names in Tier 1 core.
                            #[cfg(feature = "stdlib")]
                            if let Some(src) = crate::stdlib_vendor::stdlib_vendor_source(&path_str) {
                                let vfs_path = std::path::PathBuf::from(
                                    format!("<vendor>/{}.rb", &*path_str)
                                );
                                if let Err(t) = self.compile_and_run_source(vfs_path, src.to_string()) {
                                    return Some(Err(t));
                                }
                            }
                            //
                            // Materialise the constant shell(s)
                            // each stdlib name conventionally
                            // exposes (e.g. `URI`, `Logger`,
                            // `JSON`) so `defined?(URI)` reports
                            // "constant" and `URI.is_a?(Class)`
                            // returns true. The shell has no
                            // methods — calls into it still fail
                            // with NoMethodError, but the surface
                            // for "name exists" is now correct.
                            for (cname, is_module) in stdlib_constant_names(&path_str) {
                                let cid = self.interner.intern(cname);
                                let is_module = *is_module;
                                self.classes.entry(cid).or_insert_with(|| {
                                    std::rc::Rc::new(crate::value::Class {
                                        name: cname.to_string(),
                                        is_module,
                                        undefed: std::cell::RefCell::new(crate::intern::FxHashSet::default()),
                                        anon_serial: std::cell::Cell::new(0),
                                        ivar_shape: std::cell::RefCell::new(crate::value::IvarShape::default()),
                                        methods: std::cell::RefCell::new(crate::intern::FxHashMap::default()),
                                        singleton_methods: std::cell::RefCell::new(crate::intern::FxHashMap::default()),
                                        superclass: std::cell::RefCell::new(None),
                                        includes: std::cell::RefCell::new(Vec::new()),
                                        prepends: std::cell::RefCell::new(Vec::new()),
                                        singleton_prepends: std::cell::RefCell::new(Vec::new()),
                                        singleton_includes: std::cell::RefCell::new(Vec::new()),
                                        singleton_view: std::cell::RefCell::new(None),
                                        singleton_target: std::cell::RefCell::new(None),
                                        class_vars: std::cell::RefCell::new(crate::intern::FxHashMap::default()),
                                        consts: std::cell::RefCell::new(crate::intern::FxHashMap::default()),
                                        assigned_name: std::cell::RefCell::new(None),
                                        class_tag: None,
                                        ivars: std::cell::RefCell::new(crate::intern::FxHashMap::default()),
                                        frozen: std::cell::Cell::new(false),
                                        #[cfg(feature = "cext")]
                                        cext_alloc_func: std::cell::Cell::new(None),
                                    })
                                });
                            }
                            // Stub-class registration can change what a
                            // constant read resolves to.
                            self.bump_const_gen();
                            // CRuby's `YAML` is literally `Psych` (the
                            // same object). Mirror that: after
                            // `require "yaml"` / `"psych"`, point both
                            // constants at one shared Class so
                            // `defined?(Psych)` is true and
                            // `YAML == Psych` holds. safe_yaml's engine
                            // probe (`defined?(Psych) && YAML == Psych
                            // ? "psych" : "syck"`) then selects the
                            // modern psych branch instead of the legacy
                            // syck path (which calls `YAML.tagged_classes`).
                            if &*path_str == "yaml" || &*path_str == "psych" {
                                let yaml_id = self.interner.intern("YAML");
                                let psych_id = self.interner.intern("Psych");
                                let shared = self
                                    .classes
                                    .get(&yaml_id)
                                    .or_else(|| self.classes.get(&psych_id))
                                    .cloned();
                                if let Some(cls) = shared {
                                    self.classes.entry(yaml_id).or_insert_with(|| cls.clone());
                                    self.classes.entry(psych_id).or_insert(cls);
                                    self.bump_const_gen();
                                }
                            }
                            // Always-on extras: minimal pure-Ruby shims that
                            // ecosystem code assumes at module-load time
                            // (e.g. `URI::DEFAULT_PARSER` for rack/utils.rb).
                            // Runs after the constant shells are materialised
                            // so the shim's `module URI` reopens the same
                            // class the shells installed. Not feature-gated
                            // because Sinatra-on-rubyrs in the default build
                            // relies on it; the broader stdlib body still
                            // lives behind `--features stdlib` above.
                            if let Some(src) =
                                crate::stdlib_vendor::always_on_stub_extras(&path_str)
                            {
                                let vfs_path = std::path::PathBuf::from(
                                    format!("<vendor-extras>/{}.rb", &*path_str)
                                );
                                if let Err(t) = self.compile_and_run_source(
                                    vfs_path, src.to_string()
                                ) {
                                    return Some(Err(t));
                                }
                            }
                            Some(Ok(Value::Bool(true)))
                        } else if self.require_satisfied_by_existing_constant(&path_str) {
                            // Lenient fallback: if the namespace
                            // constant the require asks for is
                            // already defined on this Vm (either by
                            // an embedder pre-registering it or by
                            // earlier script code), treat the
                            // require as satisfied instead of going
                            // down the cext-lookup path. Lets a
                            // host pre-register `module Rack` so
                            // that `require 'rack'` (and
                            // `require 'rack/show_exceptions'`)
                            // inside a gem source no-op cleanly
                            // instead of crashing on the C-ext
                            // lookup — Rack is pure Ruby in modern
                            // versions, so the cext path would
                            // always fail for it. Mirrors the
                            // loaded_stdlib_stubs dedup pattern
                            // immediately above: Bool(true) on
                            // first observation, Bool(false)
                            // thereafter, matching CRuby's
                            // loaded-features semantics.
                            let already_loaded = self.loaded_stdlib_stubs.contains(&*path_str);
                            if already_loaded {
                                return Some(Ok(Value::Bool(false)));
                            }
                            self.loaded_stdlib_stubs.insert(path_str.to_string());
                            Some(Ok(Value::Bool(true)))
                        } else {
                            // Reached the FS-touching cext fallback —
                            // gate the sandbox here, not at the dispatch
                            // entry. Stub / satisfied-by-constant branches
                            // above are in-process and bypass the gate;
                            // under sandbox-on they let scripts use
                            // `require 'uri'`-style feature detection
                            // without tripping LoadError. `cext_require`
                            // also gates internally — this surface-level
                            // check fires first with a clearer
                            // `op = "require"` message.
                            if let Err(t) = self.check_load_allowed("require", None) {
                                return Some(Err(t));
                            }
                            #[cfg(feature = "cext")]
                            { Some(self.cext_require(&path_str)) }
                            #[cfg(not(feature = "cext"))]
                            {
                                // Match the cext-on branch's surface
                                // contract: a require-time miss is
                                // `LoadError: cannot load such file --
                                // <name>` regardless of whether the cext
                                // fallback is compiled in. Build-flag
                                // detail belongs in `--features` docs,
                                // not in a user-visible exception that
                                // `rescue LoadError` should catch.
                                Some(Err(self.trap(RubyError::LoadError {
                                    msg: format!("cannot load such file -- {}", path_str),
                                })))
                            }
                        }
                    }
                    #[cfg(target_os = "wasi")]
                    {
                        // Preserve the attempted path in the error so a
                        // script with many `require`s pinpoints which
                        // one tripped — matches the master non-wasi
                        // branch's diagnostic shape.
                        //
                        // Class is `LoadError` (not `RuntimeError`) for
                        // the same reason the native cext-on / cext-off
                        // arms above raise LoadError: a portable Ruby
                        // script using the canonical optional-require
                        // pattern
                        //
                        //     begin; require 'foo'; rescue LoadError; end
                        //
                        // must catch the miss on every target rubyrs
                        // builds for. The wasi-specific diagnostic
                        // message survives — the *class* is the
                        // load-bearing contract for `rescue`, not the
                        // text.
                        let path = path.to_string_lossy();
                        Some(Err(self.trap(RubyError::LoadError {
                            msg: format!(
                                "require: file I/O not available on \
                                 wasm32-wasi (attempted to load {})",
                                path
                            ),
                        })))
                    }
                }
                _ => Some(Err(self.trap(RubyError::ArgumentError {
                    msg: format!(
                        "require: expected 1 String arg, got {}",
                        args.len()
                    ),
                }))),
                }
            },
            // `require_relative "name"` — resolve relative to the
            // CURRENTLY-EXECUTING file's directory (not cwd), auto-
            // append `.rb`, parse + compile + dispatch the body in
            // the current Vm, track in `loaded_features` so
            // duplicate requires no-op. Returns true on first load,
            // false on a repeat.
            //
            // Spike scope deliberately small:
            //   - no load-path walking (LOAD_PATH is CRuby's, not
            //     ours); only the relative-to-current-file form.
            //   - no exception class for LoadError; missing files
            //     surface as a `RuntimeError` Trap.
            //   - no concurrency / monitor protection; rubyrs is
            //     single-threaded at the script level.
            #[cfg(not(target_os = "wasi"))]
            "require_relative" => match args {
                // `with_str_lossy` is Cow-backed: zero-alloc on
                // the valid-UTF-8 hot path, only the invalid-UTF-8
                // fallback owns a String. `to_string_lossy()` would
                // allocate unconditionally.
                [Value::Str(path)] => Some(
                    self.check_load_allowed("require_relative", None)
                        .and_then(|()| path.with_str_lossy(|s| self.require_relative(s))),
                ),
                // Distinguish type mismatch from arity: CRuby raises
                // TypeError for `require_relative :sym`, ArgumentError
                // for the wrong count. Reporting just "got 1" hides
                // which case the caller hit.
                [other] => Some(Err(self.trap(RubyError::TypeError {
                    msg: format!(
                        "no implicit conversion of {} into String",
                        other.conv_type_name()
                    ),
                }))),
                _ => Some(Err(self.trap(RubyError::ArgumentError {
                    msg: format!(
                        "wrong number of arguments (given {}, expected 1)",
                        args.len()
                    ),
                }))),
            },
            #[cfg(target_os = "wasi")]
            "require_relative" => Some(Err(self.trap(RubyError::LoadError {
                // LoadError, not RuntimeError — matches the non-wasi
                // `check_load_allowed` trap class and the
                // `Config::allow_filesystem_io` rustdoc's
                // "rescue LoadError" promise. A wasi-target script
                // doing `rescue LoadError` for "feature unavailable"
                // now catches this trap the same way it would catch
                // the sandbox cap's LoadError.
                msg: "require_relative: file I/O not available on wasm32-wasi".into(),
            }))),
            // `Kernel#load(filename [, wrap])` — re-executes the
            // Ruby source at `filename` every call (NO
            // `loaded_features` dedup; that's `require`'s
            // distinguishing semantic). The `wrap` second arg
            // would, in CRuby, run the loaded body inside an
            // anonymous module so its top-level constants don't
            // pollute the loader's scope; rubyrs Tier 1 doesn't
            // model anonymous-module scope swap and silently
            // ignores the flag — documented Tier-1 divergence in
            // SUBSET.md, same shape as `eval`'s ignored Binding
            // 2nd arg.
            //
            // Sandbox + scope-allowlist gates are identical to
            // `require`: a script that's blocked from
            // `require "foo.rb"` is also blocked from
            // `load "foo.rb"`. Path-resolution diverges from
            // `require` in two well-documented ways: (a) no
            // automatic `.rb` extension — `load "foo"` looks for
            // a literal `foo`, never `foo.rb`; (b) the as-given
            // path is the FIRST candidate (matches CRuby's "if
            // not absolute and not ./../ prefixed, search
            // $LOAD_PATH"; we share that probe path with require
            // via `ruby_source_candidates`, then strip the
            // .rb-only candidate when iterating). Returns `true`
            // on success (CRuby always returns true; require
            // returns false on second require, load doesn't have
            // a second-require concept).
            #[cfg(not(target_os = "wasi"))]
            "load" => {
                // User-override precedence. Unlike `require`, `load`
                // is genuinely common to shadow at top level — old
                // YAML-config loaders, test fixtures, and DSL prelude
                // files all `def load(path)` to repurpose the name.
                // `load` is intentionally NOT in `dispatch.rs::
                // is_builtin_name`'s "builtin always wins" set, so
                // the do_call no-recv fast path consults
                // `toplevel_methods` FIRST. By the time control
                // reaches this `builtin_call` arm, the dispatcher
                // has already proved no user `def load` exists for
                // this frame. Reflection (`defined?(load)` /
                // `__defined_method?`) reports "method" via the
                // companion arm above (`"load"` in the inline
                // builtin list) which checks toplevel_hit before
                // falling through to builtin, so the two paths
                // agree across the user-override / built-in split.
                match args {
                [Value::Str(path)] | [Value::Str(path), _] => {
                    let path_str = path.to_string_lossy().to_string();
                    // Lazy-lexer gate (`_rouge_native`): while raised,
                    // rouge.rb's eager per-lexer `Kernel::load` walk is
                    // skipped wholesale; the rouge shim demand-loads
                    // lexer files later with the gate lowered. Only the
                    // kramdown shim raises it, after verifying the
                    // on-disk rouge version matches the embedded
                    // static tables.
                    #[cfg(feature = "_rouge_native")]
                    if path_str.contains("/rouge/lexers/")
                        && crate::rouge_native::lexer_gate_active()
                    {
                        return Some(Ok(Value::Bool(true)));
                    }
                    if let Err(t) = self.check_load_allowed("load", None) {
                        return Some(Err(t));
                    }
                    // Reuse the require candidate search but drop the
                    // auto-.rb-extension candidate — `load` doesn't
                    // perform that transformation. We pass the
                    // raw `path_str` through and walk candidates;
                    // if the user wanted `.rb` resolution they
                    // included it themselves (e.g. `load "boot.rb"`
                    // is what real apps write). Absolute paths
                    // shortcut the search exactly as in require.
                    let candidates = self.ruby_source_candidates(&path_str);
                    let mut tried: Vec<String> = Vec::with_capacity(candidates.len());
                    let mut canon: Option<std::path::PathBuf> = None;
                    for c in &candidates {
                        // Skip the implicit `.rb`-appended candidate —
                        // ruby_source_candidates returns both the
                        // as-given form and a `<stem>.rb` form; load
                        // wants only the as-given matches. The .rb form
                        // is distinguishable because it has a `.rb`
                        // extension while `path_str` didn't.
                        let original_has_rb = std::path::Path::new(&path_str)
                            .extension()
                            .map(|e| e == "rb")
                            .unwrap_or(false);
                        let candidate_has_rb = c.extension().map(|e| e == "rb").unwrap_or(false);
                        if !original_has_rb && candidate_has_rb {
                            continue;
                        }
                        tried.push(c.display().to_string());
                        let Ok(resolved) = std::fs::canonicalize(c) else { continue };
                        if self.check_load_allowed("load", Some(&resolved)).is_ok() {
                            // Key by expand_path (CRuby), scope by realpath.
                            canon = Some(Self::expand_load_path(c));
                            break;
                        }
                    }
                    let canon = match canon {
                        Some(c) => c,
                        None => {
                            // `tried` is intentionally NOT in the message:
                            // CRuby's LoadError surface is just
                            // `cannot load such file -- <name>` so a
                            // `rescue LoadError => e; e.message` round-trip
                            // matches byte-for-byte. The candidate list
                            // was useful while debugging the
                            // `ruby_source_candidates` shape but it's
                            // diagnostic noise in steady state.
                            let _ = tried;
                            return Some(Err(self.trap(RubyError::LoadError {
                                msg: format!("cannot load such file -- {}", path_str),
                            })));
                        }
                    };
                    // Bypass the `loaded_features` dedup table —
                    // `load` re-executes unconditionally. We
                    // intentionally don't insert into
                    // `loaded_features` either, so a subsequent
                    // `require` on the same file would still run.
                    // That's CRuby semantics: `load` and `require`
                    // have separate cache disciplines.
                    let source = match std::fs::read_to_string(&canon) {
                        Ok(s) => s,
                        Err(e) => return Some(Err(self.trap(RubyError::LoadError {
                            msg: format!("load: read {} failed: {}", canon.display(), e),
                        }))),
                    };
                    // Drop the prior loaded_features entry (if any)
                    // so compile_and_run_source — which marks the
                    // path loaded — doesn't no-op on a re-load. The
                    // `load` contract is "always run"; the insert
                    // below temporarily marks for circular-require
                    // safety during compile, then we remove on
                    // success so the next call runs again too.
                    let was_loaded = self.loaded_features.remove(&canon);
                    let res = self.compile_and_run_source(canon.clone(), source);
                    // Always rewind the features-set state: on
                    // success we want subsequent `load` calls to
                    // re-run; on failure we shouldn't add a fake
                    // "this was loaded" entry that would silence
                    // a future require.
                    self.loaded_features.remove(&canon);
                    if was_loaded {
                        // Preserve prior require's bookkeeping so
                        // a subsequent `require` of the same path
                        // still no-ops. Without this re-insert,
                        // load + require interleave would cause
                        // require to re-execute too.
                        self.loaded_features.insert(canon);
                    }
                    Some(res.map(|_| Value::Bool(true)))
                }
                [other] | [other, _] => Some(Err(self.trap(RubyError::TypeError {
                    msg: format!(
                        "no implicit conversion of {} into String",
                        other.conv_type_name()
                    ),
                }))),
                _ => Some(Err(self.trap(RubyError::ArgumentError {
                    msg: format!(
                        "wrong number of arguments (given {}, expected 1..2)",
                        args.len()
                    ),
                }))),
                }
            }
            #[cfg(target_os = "wasi")]
            "load" => Some(Err(self.trap(RubyError::LoadError {
                msg: "load: file I/O not available on wasm32-wasi".into(),
            }))),
            // `Kernel#eval(string [, _binding, _file, _line])` —
            // parse + compile + run the source string at top
            // level. Returns the final expression's value.
            //
            // Tier 1 divergences (documented in docs/SUBSET.md):
            //   - `Binding` is not modeled — any 2nd-arg binding
            //     is silently ignored; eval'd code sees only top-
            //     level scope, not the caller's locals.
            //   - The optional `file` / `line` args are accepted
            //     for signature compatibility but only `file` is
            //     wired through to source-registration (used by
            //     backtraces / `Method#source_location`).
            "eval" => match args {
                // Arity guard FIRST so too-many-arg calls surface
                // as ArgumentError, matching CRuby's check order
                // (arity → type). Without this, `eval(123, nil,
                // "file", 1, :extra)` would TypeError on the
                // first-arg check below even though the call is
                // out of the 1..4 signature.
                _ if args.len() > 4 => {
                    Some(Err(self.trap(RubyError::ArgumentError {
                        msg: format!(
                            "wrong number of arguments (given {}, expected 1..4)",
                            args.len()
                        ),
                    })))
                }
                // Validate source arg type after the arity check.
                // Non-String first arg surfaces as TypeError.
                [other, ..] if !matches!(other, Value::Str(_)) => {
                    Some(Err(self.trap(RubyError::TypeError {
                        msg: format!(
                            "no implicit conversion of {} into String",
                            other.conv_type_name()
                        ),
                    })))
                }
                [Value::Str(src)] => {
                    let owned = src.to_string_lossy();
                    Some(self.eval_string(&owned, "(eval)", /*synthetic=*/true))
                }
                // 2-arg `eval(src, binding)`: when the 2nd arg is a
                // Binding, run with its captured self (else drop it,
                // the old divergence).
                [Value::Str(src), binding] => {
                    let owned = src.to_string_lossy();
                    match self.extract_binding_ctx(binding) {
                        Some((self_o, cctx, locals)) => Some(self.eval_string_full(
                            &owned, "(eval)", true, cctx, Some(self_o), locals, None,
                            None,
                        )),
                        None => Some(self.eval_string(&owned, "(eval)", true)),
                    }
                }
                // 3-arg / 4-arg with filename: validate file arg
                // type FIRST (CRuby raises TypeError, not
                // ArgumentError, on non-String file). Then use
                // it for source registration so backtraces point
                // at the right place (tilt passes its template
                // path here).
                [Value::Str(_), _binding, file_arg, ..]
                    if !matches!(file_arg, Value::Str(_)) => {
                    Some(Err(self.trap(RubyError::TypeError {
                        msg: format!(
                            "no implicit conversion of {} into String",
                            file_arg.conv_type_name()
                        ),
                    })))
                }
                // 4-arg with non-Integer-coercible line arg: CRuby
                // raises TypeError "no implicit conversion of X
                // into Integer". Accept Int and Float (Float has
                // `to_int` in CRuby); reject everything else even
                // though we ultimately ignore the line offset.
                // Silent acceptance would mask caller bugs.
                [Value::Str(_), _binding, Value::Str(_), line_arg]
                    if !matches!(line_arg, Value::Int(_) | Value::Float(_)) => {
                    Some(Err(self.trap(RubyError::TypeError {
                        // CRuby num2long shape: nil gets "from nil to integer"
                        // (probed vs 3.4.1); others value-word "of X into Integer".
                        msg: line_arg.num2int_conv_msg(),
                    })))
                }
                [Value::Str(src), binding, Value::Str(file)]
                | [Value::Str(src), binding, Value::Str(file), _] => {
                    let owned = src.to_string_lossy();
                    let fname = file.to_string_lossy();
                    // 4th arg (when present) is the line the source's
                    // first line maps to in backtraces.
                    let line_base = match args.get(3) {
                        Some(Value::Int(n)) => Some(*n as i32),
                        Some(Value::Float(f)) => Some(*f as i32),
                        _ => None,
                    };
                    // `synthetic=false`: caller supplied the
                    // filename explicitly. Pass through to keep
                    // `__FILE__` stable across repeated evals. A
                    // Binding 2nd arg runs with its captured self
                    // (rack's Builder.new_from_string: `eval(rackup,
                    // builder_binding, path)`).
                    match self.extract_binding_ctx(binding) {
                        Some((self_o, cctx, locals)) => Some(self.eval_string_full(
                            &owned, &fname, false, cctx, Some(self_o), locals, line_base, None,
                        )),
                        None => Some(self.eval_string_with_line(&owned, &fname, false, line_base)),
                    }
                }
                _ => Some(Err(self.trap(RubyError::ArgumentError {
                    msg: format!(
                        "wrong number of arguments (given {}, expected 1..4)",
                        args.len()
                    ),
                }))),
            },
            _ => None,
        }
    }

    /// `require_relative` host: read + parse + compile + execute the
    /// target file inline in this Vm. Path is resolved relative to
    /// the currently-executing source file's directory (mirrors
    /// CRuby) and `.rb` is appended if absent. Tracks loaded paths
    /// in `Vm.loaded_features` so a repeat call returns `false`
    /// without re-evaluation.
    ///
    /// Implementation notes:
    /// - the new file's top-level body becomes a fresh `<main>`-
    ///   shaped Proto; we push a frame for it and then run an inner
    ///   dispatch loop (`dispatch_until`) until that frame returns.
    ///   The return value (the file's last expression) is discarded
    ///   — `require_relative` returns the load-status Bool instead.
    /// - the new Proto carries fresh call-site cache slots; the Vm's
    ///   `cache_counter` advances by however many `Op::Call`s were
    ///   emitted, and `ensure_call_caches` grows the IC table to
    ///   match.
    /// - SyntaxError / IO errors surface as Trap (file-not-found
    ///   maps to RuntimeError; would be LoadError in CRuby but the
    ///   class hierarchy doesn't ship it yet).
    #[cfg(not(target_os = "wasi"))]
    pub(crate) fn require_relative(&mut self, path_str: &str) -> Result<Value, Trap> {
        use std::path::{Path, PathBuf};
        // Resolve relative to the CALL SITE's source file. CRuby's
        // contract is "the file containing the calling code" —
        // i.e., the source file the `require_relative` token
        // appears in. Each proto carries its source filename
        // (compile_proto threads `filename_rc` through), and the
        // currently-running proto is exactly the top frame's. So
        // the right anchor is `frames.last().proto.filename`
        // regardless of is_block / is_class_body.
        //
        // (Earlier rounds of this PR skipped block frames thinking
        // they'd want the enclosing method's file — that's wrong:
        // a block's proto.filename is the file the BLOCK was
        // defined in, which is the call-site file for any
        // `require_relative` lexically inside that block. Methods
        // called from a different file via `define_method` /
        // `instance_eval` would otherwise mis-anchor.)
        let anchor_filename: Option<String> = self.frames.last()
            .map(|f| self.protos[f.proto_idx].filename.to_string());
        let base_dir: PathBuf = match anchor_filename {
            Some(f) => Path::new(&f).parent().map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from(".")),
            None => PathBuf::from("."),
        };
        // Ruby appends `.rb` unless the path already names a `.rb`
        // (or native) file. A dotted basename like `maker/1.0` has
        // extension `"0"` — NOT a loadable extension — so CRuby still
        // appends `.rb` → `maker/1.0.rb` (rss requires its versioned
        // submodules this way). `set_extension("rb")` would REPLACE the
        // `.0`, yielding `maker/1.rb`, so build the candidate by string
        // append instead. A path that already ends in `.rb` is used
        // verbatim.
        let has_ruby_ext = Path::new(path_str)
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e == "rb");
        let target = if has_ruby_ext {
            base_dir.join(path_str)
        } else {
            base_dir.join(format!("{path_str}.rb"))
        };
        // `real` (realpath, symlink-resolved) probes existence and
        // backs the allowlist-scope check; `canon` (expand_path, no
        // symlink resolution) is the CRuby-faithful $LOADED_FEATURES /
        // source key. They differ only across a symlink (macOS /tmp →
        // /private/tmp).
        let real = match std::fs::canonicalize(&target) {
            Ok(p) => p,
            Err(e) => return Err(self.trap(RubyError::RuntimeError {
                msg: format!("require_relative: cannot find {} ({})", target.display(), e),
            })),
        };
        let canon = Self::expand_load_path(&target);
        // Allowlist scope: bool gate already fired at the dispatch
        // arm (check_load_allowed("require_relative", None) before
        // path string handling, F6 ordering). This second call
        // re-runs the bool gate (no-op when already passed) and
        // additionally rejects canon paths outside any configured
        // `Config::allowed_paths` prefix. Canon was already
        // symlink-resolved by `std::fs::canonicalize`, so we get
        // a true post-resolution prefix check.
        self.check_load_allowed("require_relative", Some(&real))?;
        self.load_ruby_source_from_canon(canon)
    }

    /// CRuby `File.expand_path` semantics for the require/load KEY:
    /// make the path absolute (against cwd) and resolve `.`/`..`
    /// LEXICALLY, WITHOUT resolving symlinks. CRuby keys
    /// `$LOADED_FEATURES` and `Method#source_location` this way, so on
    /// macOS `/tmp/x` stays `/tmp/x` rather than `std::fs::canonicalize`'s
    /// realpath `/private/tmp/x`. We still `canonicalize` SEPARATELY for
    /// the existence probe and the allowlist-scope check (symlink-
    /// resolved → no symlink-escape), and only use this for the visible
    /// key — so `$LOADED_FEATURES.delete("/tmp/x")` in user code (e.g.
    /// sinatra/reloader) matches what require stored.
    #[cfg(not(target_os = "wasi"))]
    fn expand_load_path(p: &std::path::Path) -> std::path::PathBuf {
        use std::path::{Component, PathBuf};
        let abs = if p.is_absolute() {
            p.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(p))
                .unwrap_or_else(|_| p.to_path_buf())
        };
        let mut out = PathBuf::new();
        for comp in abs.components() {
            match comp {
                Component::CurDir => {}
                Component::ParentDir => {
                    // Pop a Normal segment; never climb past the root.
                    if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                        out.pop();
                    }
                }
                other => out.push(other.as_os_str()),
            }
        }
        out
    }

    /// Does an existing class/module on this Vm already satisfy
    /// the given `require` path?
    ///
    /// Maps `path_str`'s first segment (everything before the
    /// first `/`) to a Ruby constant name in the two shapes
    /// embedders / scripts commonly use:
    ///
    ///   - `snake_case` → `CamelCase` (`rack` → `Rack`,
    ///     `active_record` → `ActiveRecord`)
    ///   - input itself UPPER-cased (`json` → `JSON`,
    ///     `uri` → `URI`) — captures the all-caps abbreviation
    ///     convention CRuby uses for several stdlib names
    ///
    /// Looks both up in `self.classes`. Returns true if either
    /// shape resolves to a defined class or module. Only the
    /// **first** segment is checked, so `require 'rack/cors'`
    /// matches if `Rack` exists — consistent with how Rubygems
    /// treats `<gem>/<subfile>` paths.
    ///
    /// Deliberately conservative: only fires for paths that don't
    /// match a `.rb` file or `is_stdlib_stub_name`. Used as the
    /// last fallback before `cext_require`, so it costs nothing
    /// in the happy path.
    #[cfg(not(target_os = "wasi"))]
    fn require_satisfied_by_existing_constant(&mut self, path_str: &str) -> bool {
        // Walk EVERY segment, not just the first. Reject empty,
        // `.`, `..` — those are filesystem traversal shapes that
        // should never be lenient-fallback'd against an existing
        // constant. Without this, `require "rack/../missing"`
        // would map first_seg = "rack" and silently no-op against
        // `module Rack`, bypassing the file/cext failure path
        // for filesystem-shaped require strings. Also rejects
        // `rack//foo` (empty mid-segment) and `rack/`
        // (empty trailing — though `split` includes the empty
        // trailing here).
        let segs: Vec<&str> = path_str.split('/').collect();
        if segs.is_empty() {
            return false;
        }
        // Only a SINGLE-segment require (`require "rack"`) lenient-matches
        // a same-named existing constant. A multi-segment path
        // (`require "concurrent/concurrent_ruby_ext"`) names a specific
        // sub-FILE — matching it against just its first segment's module
        // (`Concurrent`) wrongly reports success for a file that doesn't
        // exist, where CRuby raises LoadError. concurrent-ruby's native
        // loader does `require "concurrent/concurrent_ruby_ext" rescue
        // LoadError` to PICK its pure-Ruby fallback; a false success made
        // it believe the C extension loaded and then reference the
        // undefined `Concurrent::CAtomicBoolean`. CRuby-faithful: a
        // missing `a/b` require LoadErrors, it doesn't no-op against `A`.
        if segs.len() > 1 {
            return false;
        }
        for seg in &segs {
            if seg.is_empty() || *seg == "." || *seg == ".." {
                return false;
            }
        }
        let first_seg = segs[0];
        // Skip relative / absolute paths and anything that looks
        // like a real filesystem name — those should go through
        // the .rb / cext path, not this fallback.
        if first_seg.starts_with('.') || first_seg.contains('\\') {
            return false;
        }
        // Ruby constants must start with an uppercase ASCII
        // letter, which means the require-path token's first
        // segment must start with an ASCII alphabetic. A leading
        // underscore is REJECTED (not allowed-and-stripped) on
        // purpose: `snake_to_camel_case("_rack")` would otherwise
        // collapse to `Rack` (the empty segment before the first
        // `_` contributes nothing), making `require "_rack"`
        // over-match whenever `Rack` is defined. Likewise a
        // leading digit / symbol can't camelize to a valid
        // constant. Bail out so the require falls through to
        // cext_require, which will produce the same diagnostic
        // shape it would for any path with no .rb / cext sibling.
        if !first_seg.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
            return false;
        }
        // Empty-snake-segment guard. The alphabetic check above
        // only inspects the first char, so it rejects `_rack`
        // (leading underscore) but lets `rack_`, `rack__foo`,
        // and `rack_foo_` through — all of which collapse to
        // valid Ruby constant names because `snake_to_camel_case`
        // drops empty parts from its `_`-split (`["rack", ""]`
        // → capitalize each → `["Rack", ""]` → concat →
        // `"Rack"`). Without this guard a developer who mistypes
        // `require "rack_"` against an embedder-registered
        // `module Rack` would silently match, hiding the typo
        // until a later `Rack_::Something` NameError far from
        // the source of the mistake. Reject any `first_seg`
        // that yields an empty segment when split on `_` so
        // these shapes fall through to cext_require with the
        // standard diagnostic.
        if first_seg.split('_').any(|s| s.is_empty()) {
            return false;
        }
        // Core-class blocklist. `self.classes` is populated by
        // the preamble (`crates/rubyrs/src/lib.rs` ~750-1100) with
        // every built-in class/module name — `Object`, `String`,
        // `Array`, `Hash`, `Integer`, the exception hierarchy,
        // `Enumerable`, `Comparable`, `File`, `Mutex`, `Kernel`,
        // etc. Without this guard, `require "string"` would
        // silently succeed because `String` is always in
        // `self.classes`, masking a genuinely missing dependency.
        // We're only meant to fire when an EMBEDDER or earlier
        // script code registered the name; the preamble doesn't
        // count.
        //
        // The list is the union of every class/module the
        // preamble defines, normalized to lowercase for the
        // first-segment compare. Keep in sync with the preamble
        // when new core classes land. ASCII-lowercase compare
        // matches the case-insensitive walk's normalization
        // shape — `require "OBJECT"` is rejected the same as
        // `require "object"`.
        if is_core_preamble_class_name(first_seg) {
            return false;
        }
        // Interner growth guard: untrusted Ruby could otherwise
        // call `require "<unique-name>"` in a rescue loop to
        // grow the interner past `Config::max_symbols` — each
        // miss path here used to intern the camel/upper
        // candidates even when nothing in `self.classes` matched.
        // `Interner::contains()` checks for an existing entry
        // without creating one, so miss paths create no new
        // symbols; on the legitimate-match path a constant in
        // `self.classes` necessarily has its SymId already
        // interned (interning is the only way it got there),
        // so this guard never blocks a real hit.
        let camel = snake_to_camel_case(first_seg);
        if !camel.is_empty() && self.interner.contains(&camel) {
            let camel_id = self.interner.intern(&camel);
            if self.classes.contains_key(&camel_id) {
                return true;
            }
        }
        let upper = first_seg.to_ascii_uppercase();
        if upper != camel && self.interner.contains(&upper) {
            let upper_id = self.interner.intern(&upper);
            if self.classes.contains_key(&upper_id) {
                return true;
            }
        }
        // Case-insensitive fallback — covers names where Ruby's
        // canonical capitalization doesn't follow either of the
        // two shapes above. Stdlib's `IPAddr` (file `ipaddr.rb`)
        // is the textbook example: `snake_to_camel_case("ipaddr")`
        // returns `Ipaddr` and `"ipaddr".to_ascii_uppercase()`
        // returns `IPADDR`, neither of which matches. Walking the
        // classes table and ASCII-lowercase-comparing each
        // resolved name catches that. Only uses `Interner::resolve`
        // — no `intern` calls — so the symbol-cap guard above
        // doesn't need to repeat. Cost is O(n) only on
        // double-miss; n stays modest in practice (a few hundred
        // classes at most for a typical embedded Vm).
        for sym_id in self.classes.keys() {
            let name = self.interner.resolve(*sym_id);
            if name.eq_ignore_ascii_case(first_seg) {
                return true;
            }
        }
        false
    }

    /// `require "path.rb"` — load a Ruby source file by literal
    /// path (subset of CRuby's LOAD_PATH-walking `require`). Used
    /// when the cext path doesn't apply (no `.so` / `.bundle` /
    /// `.dylib` extension) and the file is a Ruby source. Path
    /// is resolved relative to cwd; `.rb` is appended if the
    /// input has no extension.
    ///
    /// This is the load-side companion to `cext_require`: the
    /// caller in `kernel.rs`'s `require` arm tries `.rb` first
    /// (if extension already says `.rb` or there's no extension
    /// and a `.rb` file exists), falling back to the cext path
    /// for native extensions. Lets pure-Ruby gem helper files
    /// (msgpack's `lib/msgpack/packer.rb` `register_type`
    /// wrapper, etc.) load cleanly without the caller having to
    /// hand-roll the wrapper.
    #[cfg(not(target_os = "wasi"))]
    pub(crate) fn require_ruby(&mut self, path_str: &str) -> Result<Value, Trap> {
        let candidates = self.ruby_source_candidates(path_str);
        // Walk candidates picking the first that BOTH canonicalizes
        // AND, when an allowlist is configured, lies inside it. The
        // earlier `find_map(canonicalize.ok())` picked the first
        // canon-success and only THEN scope-checked, so a symlink-
        // poisoned earlier candidate (e.g. `caller_dir/helpers.rb`
        // is a symlink to `/usr/share/foo/helpers.rb`) would mask a
        // legitimate later candidate (e.g. `caller_dir/../helpers.rb`
        // inside the allowlist) and trap LoadError.
        //
        // When no allowlist is configured the check_load_allowed
        // call short-circuits at the bool gate (already known true
        // here — the dispatch arm short-circuited the .rb probe
        // otherwise) and the loop matches the original behaviour.
        let mut tried: Vec<String> = Vec::with_capacity(candidates.len());
        let mut canon: Option<std::path::PathBuf> = None;
        for c in &candidates {
            tried.push(c.display().to_string());
            let Ok(resolved) = std::fs::canonicalize(c) else { continue };
            if self.check_load_allowed("require", Some(&resolved)).is_ok() {
                // Key by expand_path (CRuby), scope-check by realpath.
                canon = Some(Self::expand_load_path(c));
                break;
            }
        }
        let canon = match canon {
            Some(c) => c,
            None => {
                // No candidate satisfied both canon and scope.
                // Re-run the scope check on the first canon-
                // succeeding candidate so the caller gets the
                // descriptive LoadError when the failure is scope
                // (not "cannot find") — preserves the diagnostic
                // shape the test suite relies on.
                if let Some(resolved) = candidates.iter().find_map(|c| std::fs::canonicalize(c).ok()) {
                    self.check_load_allowed("require", Some(&resolved))?;
                    // Unreachable: check above returned Err in
                    // the loop, must return Err here too.
                    unreachable!("scope re-check changed verdict")
                }
                return Err(self.trap(RubyError::RuntimeError {
                    msg: format!("require: cannot find {} (tried: {})", path_str, tried.join(", ")),
                }));
            }
        };
        let result = self.load_ruby_source_from_canon(canon);
        // commdrv accelerator hook: the TOP-LEVEL `require "rubocop"`
        // just finished loading the real gem — RuboCop::Cop::Commissioner
        // is defined, so layer the native cop-walk driver over #walk
        // (native-first with per-walk fallback to the aliased interpreted
        // walk). The hook is `defined?(...)`-guarded, so it is inert
        // unless the host fns were registered. Inner `rubocop/...`
        // requires don't match the bare name, and a repeat require
        // returns Bool(false), so this fires exactly once per fresh load.
        // `_prism_native`-gated with the rest of the RuboCop port. (The
        // hook was already `defined?(...)`-inert without the host fns;
        // in practice a feature-off binary never even reaches here on
        // the modern stack — rubocop-ast hard-requires prism, so
        // `require "rubocop"` raises the feature-absent LoadError at
        // the `prism/prism` intercept first.)
        #[cfg(feature = "_prism_native")]
        if path_str == "rubocop" && matches!(result, Ok(Value::Bool(true))) {
            self.eval_string(
                crate::commdrv::HOOK_RB,
                "<rubyrs:commdrv_hook>",
                false,
            )?;
        }
        // `_rouge_native` accelerator hook: the TOP-LEVEL
        // `require "rouge"` just finished loading the real gem —
        // inject the shim that patches RegexLexer#lex + the HTML
        // formatter to route supported lexers through the carmine
        // engine. The shim is `defined?(...)`-guarded, so it is
        // inert unless the host fns were registered. Inner
        // `rouge/...` requires don't match the bare name, and a
        // repeat require returns Bool(false), so this fires exactly
        // once per fresh load.
        #[cfg(feature = "_rouge_native")]
        if path_str == "rouge" && matches!(result, Ok(Value::Bool(true))) {
            self.eval_string(
                crate::rouge_native::SHIM,
                "<rubyrs:rouge_native_shim>",
                false,
            )?;
        }
        // `_kramdown_native` accelerator hook. Jekyll requires
        // "kramdown-parser-gfm" once Kramdown::JekyllDocument is defined;
        // Bridgetown's own `kramdown/parser/gfm` requires it (line 3)
        // once Kramdown::BridgetownDocument is defined. The shim patches
        // whichever doc class is present and is idempotent, so we fire on
        // EVERY successful require (not just the first load): if the gem
        // was pre-required before the framework defined its document
        // subclass, the framework's later re-require still triggers the
        // patch. The shim is `defined?`-guarded — inert without the host
        // fns or a kramdown document class.
        #[cfg(feature = "_kramdown_native")]
        if path_str == "kramdown-parser-gfm" && matches!(result, Ok(_)) {
            self.eval_string(
                crate::kramdown_native::SHIM,
                "<rubyrs:kramdown_native_shim>",
                false,
            )?;
        }
        // `_liquid_native` accelerator hook, same shape: by the time
        // the TOP-LEVEL `require "jekyll"` finishes,
        // Jekyll::LiquidRenderer::File is defined and the shim can
        // patch it. `defined?(...)`-guarded, inert outside Jekyll.
        #[cfg(feature = "_liquid_native")]
        if path_str == "jekyll" && matches!(result, Ok(Value::Bool(true))) {
            self.eval_string(
                crate::liquid_native::SHIM,
                "<rubyrs:liquid_native_shim>",
                false,
            )?;
        }
        // `_yaml_native` read-phase hook, same shape: the front-matter
        // shim patches Document#read_content / Convertible#read_yaml
        // once Jekyll has defined them. `defined?(...)`-guarded,
        // inert outside Jekyll.
        #[cfg(feature = "_yaml_native")]
        if path_str == "jekyll" && matches!(result, Ok(Value::Bool(true))) {
            self.eval_string(
                crate::yaml_native::FRONTMATTER_SHIM,
                "<rubyrs:yaml_native_frontmatter_shim>",
                false,
            )?;
        }
        result
    }

    /// Search-path candidates for `require <path_str>`. First
    /// existing one wins. Matches CRuby's `require`: walks
    /// `$LOAD_PATH` only — the caller source file's directory
    /// is `require_relative`'s job, not `require`'s.
    /// (Co-located trees / cross-package hops opt in by
    /// `$LOAD_PATH.unshift(dir)` at boot; require_xpkg
    /// fixture is the in-tree example.) Absolute paths
    /// shortcut the search.
    ///
    /// Order:
    ///   1. as-given (handles absolute paths + cwd-relative).
    ///   2. each `$LOAD_PATH` entry + name.rb (in order;
    ///      scripts opt into this by `$LOAD_PATH.unshift(dir)`
    ///      at boot).
    ///   3. raw input as last-resort defensive fallback when
    ///      auto-`.rb` extension was applied but didn't match.
    ///
    /// (Pass-10 layer #6 removed the caller_dir + caller_dir
    /// parent candidates — they shadowed the stdlib-stub
    /// fallback when a `require` inside an already-loaded
    /// file resolved back to that same file. See PR #295.)
    ///
    /// Shared by `require_ruby` (for the actual load) and the
    /// `require` dispatch arm (for the .rb-vs-cext routing
    /// decision) so the two stay structurally guaranteed to
    /// agree on which candidates to consider.
    #[cfg(not(target_os = "wasi"))]
    pub(crate) fn ruby_source_candidates(&self, path_str: &str) -> Vec<std::path::PathBuf> {
        use std::path::{Path, PathBuf};
        let p = Path::new(path_str);
        // CRuby's `require` APPENDS `.rb` to the feature name unless it
        // already ends in `.rb` — it does NOT treat an arbitrary trailing
        // dotted segment as a pre-existing extension to keep or replace.
        // `require "rss/1.0"` must look for `rss/1.0.rb`. The old
        // `p.extension().is_none()` check saw `.0` as an extension and
        // left the name unsuffixed (so `rss/1.0.rb` was never tried);
        // worse, `with_extension("rb")` would have REPLACED `.0` →
        // `rss/1.rb`. Append by string, gated only on a literal `.rb`
        // suffix. (Native `.so`/`.bundle` requests are routed through the
        // separate cext-candidate path, not here.)
        let rb_form: PathBuf = if path_str.ends_with(".rb") {
            p.to_path_buf()
        } else {
            PathBuf::from(format!("{}.rb", path_str))
        };
        let mut candidates: Vec<PathBuf> = Vec::with_capacity(4);
        candidates.push(rb_form.clone());
        if !rb_form.is_absolute() {
            // CRuby's `require` walks `$LOAD_PATH` ONLY — the
            // caller's directory is `require_relative`'s job,
            // not `require`'s. Pre-fix this candidate list
            // also included the caller's directory (and its
            // parent), which broke the stdlib-stub fallback
            // for nested `require`s: e.g. `require "tilt/erb"`
            // runs lib/tilt/erb.rb whose body does
            // `require 'erb'`; the caller_dir resolution
            // matched tilt/erb.rb itself (already in
            // loaded_features) → returned Bool(false) without
            // ever reaching `is_stdlib_stub_name`. Result: the
            // ERB constant never got installed, and subsequent
            // `::ERB` lookups raised NameError. (TRY_RUNS
            // pass-10 layer #6.)
            //
            // `$LOAD_PATH` walk. The Array is populated by the
            // script's own `$LOAD_PATH.unshift(dir)` calls; if
            // it was never touched (lazy `Vm.load_path` still
            // `None`) this is a zero-cost no-op.
            if let Some(lp_id) = self.load_path {
                for entry in self.heap.array(lp_id).iter() {
                    if let Value::Str(s) = entry {
                        let dir = s.to_string_lossy();
                        candidates.push(PathBuf::from(dir).join(&rb_form));
                    }
                }
            }
        }
        if rb_form != p {
            candidates.push(p.to_path_buf());
        }
        candidates
    }

    /// Quick existence probe — true iff `path_str` resolves to a
    /// real Ruby source file under the search-path rules above.
    /// Used by the require routing to pick the `.rb` path over
    /// the cext path when both could in principle apply.
    #[cfg(not(target_os = "wasi"))]
    pub(crate) fn find_ruby_source_candidate(&self, path_str: &str) -> bool {
        // A KNOWN native-library extension (.so/.dylib/…) routes
        // straight to cext, skipping .rb candidates. But an arbitrary
        // dotted trailing segment is NOT an extension: `require
        // "rss/1.0"` must look for `rss/1.0.rb` (CRuby appends `.rb`
        // unless the name already names a loadable file). The old `ext
        // != "rb"` check mis-treated the `.0` as an extension and
        // routed it to cext → LoadError. Bail only for real native
        // suffixes.
        let p = std::path::Path::new(path_str);
        if let Some(ext) = p.extension().and_then(|e| e.to_str())
            && matches!(ext, "so" | "bundle" | "dylib" | "dll" | "o") {
            return false;
        }
        // When `allowed_paths` is configured, skip the `.exists()`
        // probe for candidates whose lexically-resolved path lies
        // outside every prefix — closes a stat side-channel where
        // a script-controlled `require` argument could probe
        // arbitrary host paths via timing/error-shape. Candidates
        // inside scope still probe normally; the downstream
        // `require_ruby` re-runs the canonicalize-then-scope check.
        let in_scope = |c: &std::path::Path| -> bool {
            let Some(prefixes) = self.allowed_paths.as_ref() else {
                return true;
            };
            let joined = if c.is_absolute() {
                c.to_path_buf()
            } else {
                match std::env::current_dir() {
                    Ok(cwd) => cwd.join(c),
                    Err(_) => c.to_path_buf(),
                }
            };
            let resolved = crate::lexically_resolve_path(&joined);
            prefixes.iter().any(|pfx| resolved.starts_with(pfx))
        };
        self.ruby_source_candidates(path_str)
            .iter()
            .any(|c| in_scope(c) && c.exists())
    }

    /// Shared load body for `require` / `require_relative` once
    /// the canonical path is resolved. Handles loaded-features
    /// dedup, source read, parse, compile, frame push, dispatch
    /// until completion. Returns `Bool(true)` on first load,
    /// `Bool(false)` on a repeat, and propagates parse / load
    /// errors as `Trap`.
    #[cfg(not(target_os = "wasi"))]
    pub(crate) fn load_ruby_source_from_canon(
        &mut self,
        canon: std::path::PathBuf,
    ) -> Result<Value, Trap> {
        if self.loaded_features.contains(&canon) {
            // Mid-load circular re-require (canon is loading but not yet
            // completed): always dedup — re-loading would recurse.
            if !self.completed_features.contains(&canon) {
                return Ok(Value::Bool(false));
            }
            // Completed before. Honor the dev-reload idiom
            // `$LOADED_FEATURES.delete(path); require path` (sinatra/
            // reloader, Rails): dedup only while canon is still in the
            // script-visible $LOADED_FEATURES array. If the user removed
            // it, drop the internal markers and fall through to re-load.
            if self.script_loaded_features_contains(&canon) {
                return Ok(Value::Bool(false));
            }
            self.loaded_features.remove(&canon);
            self.completed_features.remove(&canon);
        }
        let source = match std::fs::read_to_string(&canon) {
            Ok(s) => s,
            Err(e) => return Err(self.trap(RubyError::RuntimeError {
                msg: format!("require: read {} failed: {}", canon.display(), e),
            })),
        };
        self.compile_and_run_source(canon, source)
    }

    /// Body shared by `load_ruby_source_from_canon` (disk source)
    /// and the `stdlib` feature's vendor require path (embedded
    /// `include_str!` source). Takes a pre-read source string plus
    /// a path used as the loaded_features key, filename tracking
    /// key, and backtrace label. Caller owns the "did we already
    /// load this?" guard; this helper unconditionally runs the body.
    #[cfg(not(target_os = "wasi"))]
    pub(crate) fn compile_and_run_source(
        &mut self,
        canon: std::path::PathBuf,
        source: String,
    ) -> Result<Value, Trap> {
        // Parse + AST translate. Errors surface as SyntaxError
        // through the standard Trap path.
        let parse_result = ruby_prism::parse(source.as_bytes());
        let mut parse_errors = parse_result.errors().peekable();
        if parse_errors.peek().is_some() {
            let msg = crate::error::format_prism_errors(&source, parse_errors);
            return Err(self.trap(RubyError::SyntaxError { msg }));
        }
        let (prog, ast_errors) = crate::ast::tr_with_errors_on_source(
            &parse_result.node(),
            parse_result.source(),
        );
        if !ast_errors.is_empty() {
            return Err(self.trap(RubyError::SyntaxError {
                msg: ast_errors.join("; "),
            }));
        }
        let filename_rc: std::rc::Rc<str> = std::rc::Rc::from(canon.to_string_lossy());
        // Register the loaded source so `Method#source_location` and
        // any other Vm-side byte-offset → line/col resolver can find
        // it. `Runtime::eval` does the same for top-level scripts; we
        // must mirror it here, otherwise methods defined inside the
        // required file lose backtrace fidelity.
        let source_rc: std::rc::Rc<str> = std::rc::Rc::from(source.as_str());
        self.sources.insert(filename_rc.clone(), source_rc);
        // Mark loaded BEFORE running the body — matches CRuby's
        // semantics for circular requires (mid-load is treated as
        // "already loading"; the partially-defined module is visible
        // to the re-entrant require).
        self.loaded_features.insert(canon.clone());
        // CRuby: a file marked "loading" SATISFIES any autoload registered for
        // it — `autoload?` returns nil from this point on (so a file doing
        // `autoload(:Self, __FILE__)` and checking autoload? mid-body sees nil,
        // and one up the require chain is ignored too). Consume the matching
        // autoloads NOW (at load start), via the O(1) reverse map — the
        // canonicalize happened once at registration, not in this hot path. The
        // const isn't defined yet, so leave a removable undef-slot; StoreConst
        // clears it if the body defines the constant.
        #[cfg(not(target_os = "wasi"))]
        if let Some(keys) = self.autoload_paths.remove(&canon) {
            for k in keys {
                let removed = self.autoloads_scoped.remove(&k).is_some()
                    | self.autoloads_toplevel.remove(&k).is_some();
                if removed
                    && !self.classes.contains_key(&k)
                    && !self.constants.contains_key(&k)
                {
                    self.consumed_autoloads.insert(k);
                }
            }
        }
        // NOTE: the script-visible `$LOADED_FEATURES` Array is pushed on
        // SUCCESSFUL COMPLETION below (CRuby order), NOT here — so a
        // nested `require` inside this body sees the just-completed inner
        // file as `$LOADED_FEATURES.last`, not this still-loading outer
        // one. zeitwerk's decorated `require` reads `.last` to identify
        // the file it loaded; pushing the outer file early made a nested
        // `require "time"` mid-load misidentify (firing on_file_autoloaded
        // for the OUTER file prematurely → "expected X to define Y but
        // didn't"). The `loaded_features` Set above stays the require-
        // dedup authority and IS marked before the body (circular-require
        // semantics unchanged).
        let fsl_start = self.protos.len();
        let entry = crate::compiler::compile_proto(
            "<require>".into(), vec![], &[prog], filename_rc,
            &mut self.protos, &mut self.interner, &mut self.cache_counter,
        );
        if crate::compiler::detect_frozen_string_literal(&source) {
            crate::compiler::mark_frozen_string_literal(&mut self.protos, fsl_start);
        }
        let cc = self.cache_counter.call as usize;
        let ivc = self.cache_counter.ivar as usize;
        self.ensure_ivar_caches(ivc);
        self.ensure_call_caches(cc);
        // Push a fresh top-level frame for the loaded body and run
        // the inner dispatch loop until it returns. dispatch_until
        // is the same helper iterator drivers use to run a block
        // body without unwinding the outer dispatch.
        //
        // Capture the stack depth at push time so we can later
        // distinguish "my frame popped via Op::Return" (stack ends
        // at stack_before + 1) from "outer rescue unwound past my
        // frame" (stack truncated to some prior frame's base_sp,
        // which is <= stack_before). Both look identical from a
        // frames.len() perspective once dispatch_until returns Ok.
        let depth_before = self.frames.len();
        let stack_before = self.stack.len();
        // A required file's body runs at top-level lexical nesting, like
        // CRuby — its `def`s land on Object (a private global function),
        // NOT on whatever class body the `require` call sits inside.
        // `Op::DefMethod` resolves its target from `class_stack.last()`,
        // so we must hide the caller's class-body context (and the
        // parallel visibility / module_function stacks) for the duration
        // of the required body, restoring them on every exit path below.
        // Without this, `require "delegate"` from inside a `class Foo`
        // body defined `DelegateClass` on `Foo` instead of Object —
        // mustermann's `class NodeTranslator < DelegateClass(Node)`
        // (loaded inside Hanami::Router's class body) then raised
        // NoMethodError.
        let saved_class_stack = std::mem::take(&mut self.class_stack);
        let saved_visibility_stack = std::mem::take(&mut self.class_visibility_stack);
        let saved_modfn_stack = std::mem::take(&mut self.module_function_active_stack);
        // A required file's top-level `self` is the `main` object, like
        // CRuby — so top-level `self.extend Module` in a required file
        // works (rake/dsl_definition.rb:196 `self.extend Rake::DSL`).
        let main_self = self.main_object();
        self.frames.push(super::Frame {
            proto_idx: entry,
            ip: 0,
            locals: crate::vm::Locals::Shared(std::rc::Rc::new(std::cell::RefCell::new(
                super::vec_nil(self.protos[entry].n_locals as usize)
            ))),
            self_val: main_self,
            base_sp: self.stack.len(),
            is_class_body: false,
            swap_return: None,
            block_arg: None,
            defining_class: None,
            lexical_cvar_class: None,
            #[cfg(feature = "regex")] saved_last_match: None,
            is_block: false, is_lambda: false,
            n_given_positional: 0,
            kw_given_mask: 0,
            aux: None,
            pending_yield: false,
            block_writeback: None,
            dm_share: false,
            own_start: 0,
            outer_cell_start: 0,
            outer_cell: None,
            outer_rest: None,
            captured_yield_block: None,
        });
        // Dispatch loop. We can't just call `dispatch_until` and
        // bail on the first method_return: a non-local `return`
        // INSIDE the required file (e.g. `def helper;
        // arr.each { return }; end; helper`) targets a method
        // defined WITHIN the file and should unwind locally,
        // letting the rest of the file keep loading. Only escape
        // when the unwind would pop our pushed <main> frame.
        //
        // Structure mirrors `Vm::dispatch`'s loop body around the
        // method_return arm, but with a depth cap so we stop at
        // `depth_before` instead of `frames.is_empty()`.
        loop {
            // Step until method_return fires or we drop to
            // depth_before (normal completion / outer rescue
            // unwound past us).
            if let Err(trap) = self.dispatch_until(depth_before) {
                self.loaded_features.remove(&canon);
                self.class_stack = saved_class_stack;
                self.class_visibility_stack = saved_visibility_stack;
                self.module_function_active_stack = saved_modfn_stack;
                return Err(trap);
            }
            if self.method_return.is_none() {
                // Either Op::Return on our <main> (frames at
                // depth_before with value on stack), or outer
                // rescue unwound below (frames < depth_before).
                // The stack-length check below distinguishes.
                break;
            }
            // method_return is set; mimic dispatch's unwind. If it
            // stays within the required file (frames > depth_before
            // after unwind), continue dispatching. If the unwind
            // would pop OUR <main> frame or beyond, the return
            // escapes — bail with suppress flag.
            // Use `take_method_return` (vm.rs) so the
            // `pending_loop_transfer` invariant — a non-local
            // return supersedes a mid-ensure break/next — is
            // applied at the same instant we consume the value,
            // mirroring `step.rs::dispatch`'s unwind arm. The
            // restore-on-escape branch below (line ~1198) puts
            // `method_return` back without restoring
            // `pending_loop_transfer`; that's correct — the
            // structured transfer was already invalidated by the
            // non-local return crossing this take site, and the
            // outer dispatch loop will re-take and re-apply the
            // invariant via the same helper if the unwind keeps
            // climbing.
            let owner_rc = self.method_return_locals.clone();
            let val = self.take_method_return().unwrap();
            // Lexical-aware unwind, mirroring step.rs's dispatch
            // arm but capped at `depth_before + 1` so the
            // lexical-owner method frame can never be our pushed
            // <main> (that case is "the return escapes the
            // required file"). (TRY_RUNS pass-10 layer #4.)
            let mut escaped = false;
            loop {
                if self.frames.len() <= depth_before + 1 {
                    // Popping the next frame would take us at or
                    // below the require sentinel — the return
                    // escapes this `require` boundary.
                    escaped = true;
                    break;
                }
                let f_ref = self.frames.last().unwrap();
                let is_owner = !f_ref.is_block && match &owner_rc {
                    Some(rc) => f_ref
                        .locals
                        .as_shared()
                        .is_some_and(|l| std::rc::Rc::ptr_eq(l, rc)),
                    None => true,  // legacy fallback
                };
                let f = self.frames.pop().unwrap();
                self.stack.truncate(f.base_sp);
                if f.dm_share {
                    self.dm_share_depth = self.dm_share_depth.saturating_sub(1);
                }
                if f.is_class_body {
                    let cls = self.class_stack.pop()
                        .expect("ICE: class_stack empty unwinding through class_eval (require/_relative)");
                    self.class_visibility_stack.pop();
                    self.module_function_active_stack.pop();
                    if is_owner {
                        self.stack.push(Value::Class(cls));
                    }
                } else if is_owner {
                    if let Some(r) = f.swap_return {
                        self.stack.push(r);
                    } else {
                        self.stack.push(val.clone());
                    }
                }
                self.release_frame_locals(f.locals);
                if is_owner { break; }
            }
            if escaped {
                self.method_return = Some(val);
                self.sync_control_signals();
                self.method_return_locals = owner_rc;
                break;
            }
            // Continue dispatching at the method's caller (still
            // inside required file body since we capped at
            // depth_before + 1).
        }
        // Restore the caller's definee stacks now that the required
        // body has finished (normally or via an unwind that stayed
        // below our <main> frame). All remaining exit paths are past
        // this point, so a single restore covers them.
        self.class_stack = saved_class_stack;
        self.class_visibility_stack = saved_visibility_stack;
        self.module_function_active_stack = saved_modfn_stack;
        // If method_return is still set, the unwind targeted our
        // <main> or above — let the outer dispatch finish it.
        if self.method_return.is_some() {
            self.loaded_features.remove(&canon);
            self.suppress_call_result_push = true;
            return Ok(Value::Nil);
        }
        // Outer-rescue-unwound-past-us case. When an exception
        // raised inside the required file is caught by a `rescue`
        // in OUR caller (or further up), `unwind_with_exception`
        // truncates the operand stack to the handler frame's
        // `base_sp` and re-routes its IP. dispatch_until then
        // exits via the loop condition `frames.len() > until_depth`
        // becoming false (the caller's frame is at <= depth_before
        // now), returning Ok(()). At this point the operand stack
        // is the rescue handler's, NOT ours — popping or pushing
        // would corrupt it (overwrite the bound exception, smash
        // saved values). The signal is that the stack didn't end
        // at `stack_before + 1` (which is what Op::Return leaves
        // behind).
        //
        // Set `suppress_call_result_push` so do_call's builtin arm
        // skips its `stack.push(builtin_result)` step — otherwise
        // it'd add one slot to a stack the compiler expects at
        // exactly the rescue handler's saved `base_sp`. The Nil
        // we return is just a placeholder (the flag suppresses
        // its push); the rescue handler resumes from its own ip
        // with its own stack intact.
        if self.stack.len() != stack_before + 1 {
            self.loaded_features.remove(&canon);
            self.suppress_call_result_push = true;
            return Ok(Value::Nil);
        }
        // Normal completion: the required file's last expression
        // sits on top of the operand stack (Op::Return pushed it
        // before the frame popped). Discard — `require_relative`
        // returns the load-status Bool.
        let _ = self.stack.pop();
        // Mirror the loaded path into the script-visible
        // `$LOADED_FEATURES` Array now that the body has SUCCESSFULLY
        // run — so completion order matches CRuby and a nested require
        // (above) saw the inner file as `.last`, not this one. zeitwerk's
        // `Kernel#require` wrapper reads `$LOADED_FEATURES.last`; its
        // unload path does `$LOADED_FEATURES.reject! { … }`. Dedup by
        // path string so a `load` re-run doesn't append a duplicate.
        {
            let path_str = canon.to_string_lossy().into_owned();
            let lf_id = self.ensure_loaded_features_list()?;
            let already = matches!(self.heap.get(lf_id), HeapObj::Array(arr)
                if arr.iter().any(|v| matches!(v, Value::Str(s) if s.borrow().as_slice() == path_str.as_bytes())));
            if !already {
                let sval = Value::new_str(path_str);
                if let HeapObj::Array(arr) = self.heap.get_mut(lf_id) {
                    arr.push(sval);
                }
            }
        }
        // Mark COMPLETED (body ran to the end) so a later forced reload
        // (`$LOADED_FEATURES.delete`) can be told apart from a still-
        // loading circular re-require in the dedup check above.
        self.completed_features.insert(canon.clone());
        Ok(Value::Bool(true))
    }

    /// True if `canon` (a completed feature's path) is still present in
    /// the script-visible `$LOADED_FEATURES` array — i.e. the user has
    /// NOT removed it to force a reload. On any error reading the array
    /// we assume present (dedup), preserving the prior no-reload
    /// behavior. `#[cfg]`-gated alongside the require machinery.
    #[cfg(not(target_os = "wasi"))]
    fn script_loaded_features_contains(&mut self, canon: &std::path::Path) -> bool {
        let path_str = canon.to_string_lossy();
        let Ok(lf_id) = self.ensure_loaded_features_list() else { return true };
        matches!(self.heap.get(lf_id), HeapObj::Array(arr)
            if arr.iter().any(|v| matches!(v, Value::Str(s)
                if s.borrow().as_slice() == path_str.as_bytes())))
    }

    /// `Kernel#eval(string)` / `Class#class_eval(string, ...)`
    /// — runtime parse + compile + run of a Ruby source string.
    /// Returns the final expression's value (matching CRuby's
    /// `eval`).
    ///
    /// Unlike `compile_and_run_source` (which is `require`-
    /// flavoured), this:
    ///   - skips `loaded_features` tracking — eval'd strings
    ///     aren't files;
    ///   - returns the top-of-stack expression value instead of
    ///     `Bool(true)`;
    ///   - is NOT wasi-gated (no filesystem dependency).
    ///
    /// Source is registered in `self.sources` so backtraces and
    /// `Method#source_location` for methods defined inside the
    /// eval'd source resolve.
    ///
    /// Tier 1 divergence (consumer of this helper):
    ///   `Class#class_eval(string)` is wired to call this as
    ///   if it were a top-level eval — it does NOT switch the
    ///   class-body context to the receiver class. That means
    ///   bare `Foo.class_eval("def bar; end")` lands `bar` at
    ///   top level, not on `Foo`. The motivating consumer
    ///   (tilt-2.7.0) self-wraps the string in a NESTED
    ///   block-form `class_eval do def ... end end`, so its
    ///   defs land correctly via the existing block-form path
    ///   in `dispatch.rs`. Documented in docs/SUBSET.md.
    /// `synthetic` distinguishes our own default labels (`(eval)` /
    /// `(class_eval)`) from caller-supplied filenames. Only the
    /// synthetic case opts into the `:N` collision-suffix dedupe;
    /// explicit user filenames pass through unchanged so `__FILE__`
    /// stays stable across repeated evals — including the edge case
    /// where the caller deliberately passes the literal default
    /// string as a filename.
    /// If `v` is a `Kernel#binding`-produced Binding instance, return
    /// its captured `(self, lexical_class, locals)` for
    /// `eval(src, binding)`. `locals` is the snapshot of the capturing
    /// frame's named locals (empty when none were live). `None` for any
    /// other value (including the old inert Binding).
    fn extract_binding_ctx(&mut self, v: &Value) -> Option<BindingCtx> {
        let Value::Object(id) = v else { return None };
        let self_sym = self.interner.intern("@__self");
        let lex_sym = self.interner.intern("@__lexical_class");
        let locals = self
            .binding_locals
            .get(&(id.0 as usize))
            .cloned()
            .unwrap_or_default();
        match self.heap.get(*id) {
            crate::heap::HeapObj::Instance(inst) if inst.class.name == "Binding" => {
                let self_o = inst.ivar_get(self_sym).cloned()?;
                let cctx = match inst.ivar_get(lex_sym) {
                    Some(Value::Class(c)) => Some(c.clone()),
                    _ => None,
                };
                Some((self_o, cctx, locals))
            }
            _ => None,
        }
    }

    /// Snapshot the current (caller) frame's NAMED locals as
    /// slot-ordered `(name, value)` pairs — the local binding a
    /// string-form `class_eval` / `module_eval` runs against (CRuby
    /// gives the eval'd source the caller's local scope). Same walk the
    /// `Kernel#binding` builtin uses; `frames.last()` is the caller
    /// because class_eval is handled inline in `do_call`, not as a
    /// method with its own frame.
    pub(crate) fn snapshot_caller_named_locals(&self) -> Vec<(String, Value)> {
        let mut snap: Vec<(String, Value)> = Vec::new();
        if let Some(frame) = self.frames.last() {
            let proto_idx = frame.proto_idx;
            let shared = frame.locals.as_shared().cloned();
            let stack_base = match &frame.locals {
                crate::vm::Locals::Stack(b) => Some(*b as usize),
                crate::vm::Locals::Shared(_) => None,
            };
            // Capture routing for a block frame's outer slots
            // (mirror of Op::LoadLocal).
            let n = self.protos[proto_idx].n_locals as usize;
            for slot in 0..n {
                let name = self.protos[proto_idx]
                    .local_names
                    .get(slot)
                    .cloned()
                    .unwrap_or_default();
                if name.is_empty() {
                    continue;
                }
                let val = if let Some(cell) = frame.outer_cell_for(slot) {
                    cell.borrow().get(slot).cloned().unwrap_or(Value::Nil)
                } else if let Some(rc) = &shared {
                    rc.borrow().get(slot).cloned().unwrap_or(Value::Nil)
                } else if let Some(base) = stack_base {
                    self.locals_arena.get(base + slot).cloned().unwrap_or(Value::Nil)
                } else {
                    Value::Nil
                };
                snap.push((name, val));
            }
        }
        snap
    }

    pub(crate) fn eval_string(
        &mut self,
        source: &str,
        filename: &str,
        synthetic: bool,
    ) -> Result<Value, Trap> {
        self.eval_string_with_class_ctx(source, filename, synthetic, None)
    }

    /// `eval_string` with an optional CLASS CONTEXT: string-form
    /// `cls.class_eval(src)` runs its toplevel with self = cls,
    /// is_class_body = true, and cls on class_stack — `def` inside
    /// lands on cls's table (CRuby semantics; previously a
    /// documented toplevel-landing divergence). minitest's
    /// infect_an_assertion defines every must_*/wont_* this way —
    /// the LAST class_eval used to win globally, shadowing the
    /// deprecated Object shim with the ctx-reading Expectation
    /// body ("ctx for Integer").
    pub(crate) fn eval_string_with_class_ctx(
        &mut self,
        source: &str,
        filename: &str,
        synthetic: bool,
        class_ctx: Option<std::rc::Rc<crate::value::Class>>,
    ) -> Result<Value, Trap> {
        self.eval_string_full(source, filename, synthetic, class_ctx, None, vec![], None, None)
    }

    /// `eval_string` carrying an explicit backtrace line base (the
    /// 4th arg of `eval(src, nil, file, line)` when no Binding is
    /// supplied).
    pub(crate) fn eval_string_with_line(
        &mut self,
        source: &str,
        filename: &str,
        synthetic: bool,
        line_base: Option<i32>,
    ) -> Result<Value, Trap> {
        self.eval_string_full(source, filename, synthetic, None, None, vec![], line_base, None)
    }

    /// `eval_string_with_class_ctx` plus a `self_override` — the eval'd
    /// toplevel runs with `self` set to it. Backs `eval(src, binding)`:
    /// a Binding captures the calling scope's `self`, and method calls
    /// in the eval'd source (rack's Builder `new_from_string` evals a
    /// rackup script that calls `run`/`use`/`map` on the builder) must
    /// dispatch against that self. (Outer LOCAL-variable capture is a
    /// follow-up; this is the self-dispatch layer.)
    /// Parse + translate an eval source into a body `SExpr`, applying
    /// the Binding-locals wrap when `local_seed` is non-empty. Returns
    /// `(prog, compile_params, effective_seed, registered_source)`:
    /// - `prog` is the body to compile (the lambda's spliced body when
    ///   seeded, else the raw program).
    /// - `compile_params` pre-declares the seeded local names as
    ///   leading slots (empty when unseeded / on fallback).
    /// - `effective_seed` is the snapshot to write into those slots
    ///   (empty on fallback so we never seed mismatched slots).
    /// - `registered_source` is the text whose spans the compiled
    ///   bytecode indexes into (the wrapped source when seeded).
    ///
    /// `Err(msg)` is a hard SyntaxError from the UNSEEDED parse; a
    /// failed wrap silently degrades to the unseeded path.
    fn prepare_eval_body(
        source: &str,
        local_seed: &[(String, Value)],
    ) -> Result<EvalBody, String> {
        fn parse_tr(src: &str) -> Result<crate::ast::SExpr, String> {
            let pr = ruby_prism::parse(src.as_bytes());
            let mut errs = pr.errors().peekable();
            if errs.peek().is_some() {
                return Err(crate::error::format_prism_errors(src, errs));
            }
            let (prog, ast_errors) =
                crate::ast::tr_with_errors_on_source(&pr.node(), pr.source());
            if !ast_errors.is_empty() {
                return Err(ast_errors.join("; "));
            }
            Ok(prog)
        }
        // Splice the body out of a single top-level lambda literal —
        // either the bare `Expr::Lambda` or a one-statement `__seq__`
        // wrapping it.
        fn lambda_body(prog: &crate::ast::SExpr) -> Option<Vec<crate::ast::SExpr>> {
            use crate::ast::Expr;
            match &prog.node {
                Expr::Lambda { body, .. } => Some(body.clone()),
                Expr::Call { receiver: None, name, args, .. }
                    if name == "__seq__" && args.len() == 1 =>
                {
                    if let Expr::Lambda { body, .. } = &args[0].node {
                        Some(body.clone())
                    } else {
                        None
                    }
                }
                _ => None,
            }
        }
        if !local_seed.is_empty() {
            let names: Vec<String> = local_seed.iter().map(|(n, _)| n.clone()).collect();
            // Place the source on the SAME line as the lambda header
            // (no leading newline) so its line numbers are preserved —
            // `__LINE__` and backtrace lines stay correct (rack's
            // Builder.parse_file checks `__LINE__`). A leading UTF-8
            // BOM is stripped first: CRuby's eval ignores one, and the
            // wrap would otherwise shove it mid-source where prism
            // can't (rack's BOM rackup fixture). The trailing `\n}`
            // closes on its own line so a source ending in a line
            // comment doesn't swallow the brace. (A magic comment on
            // the source's first line is still displaced past column 0
            // and thus ignored — a documented eval-in-binding gap, not
            // regressed by this wrap.)
            let body_src = source.strip_prefix('\u{feff}').unwrap_or(source);
            let wrapped = format!("->({}) {{ {}\n}}", names.join(", "), body_src);
            if let Ok(prog) = parse_tr(&wrapped)
                && let Some(body) = lambda_body(&prog)
            {
                return Ok((crate::ast::seq(body), names, local_seed.to_vec(), wrapped));
            }
            // Fall through: the wrap failed (illegal param name, etc.).
        }
        let prog = parse_tr(source)?;
        Ok((prog, vec![], vec![], source.to_string()))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn eval_string_full(
        &mut self,
        source: &str,
        filename: &str,
        synthetic: bool,
        class_ctx: Option<std::rc::Rc<crate::value::Class>>,
        self_override: Option<Value>,
        // Named locals captured by the `Binding` (slot-ordered, parallel
        // names+values). When non-empty they become the eval proto's
        // leading params (so the compiler resolves those identifiers as
        // local reads, not method calls) and seed the eval frame's
        // locals. Empty for bare `eval` / `class_eval`.
        local_seed: Vec<(String, Value)>,
        // The line number the eval'd source's FIRST line should report
        // as in backtraces — the 3rd arg of `class_eval(src, file,
        // line)` / 4th of `eval(src, b, file, line)`. `None` ⇒ leave
        // the default (`1`, no adjustment).
        line_base: Option<i32>,
        // Encoding of the eval'd source string when it wasn't UTF-8
        // (the source's Ruby encoding tag). `None` ⇒ UTF-8 default.
        // Stamped onto the compiled proto range so string literals are
        // re-tagged to the source's encoding at load.
        source_encoding: Option<crate::value::EncodingTag>,
    ) -> Result<Value, Trap> {
        // Fast-fail BEFORE any parse / AST / compile work when
        // the frame cap is already exhausted. CPU-bound parse of
        // a large untrusted eval string shouldn't run just to
        // fail at the frame push at the bottom.
        self.check_frames()?;
        // When the eval carries Binding-captured locals, parse the
        // source WRAPPED in a lambda whose params are those local
        // names — that makes prism resolve bare `foo` references as
        // local-variable reads (its local-vs-method decision is made
        // at parse time and can't be retrofitted post-AST). We then
        // splice out the lambda's body and compile it standalone with
        // the same names as leading params; the frame's slots
        // 0..N are seeded from the snapshot below. On any wrap/parse
        // failure (e.g. a capture name that isn't a legal param) we
        // fall back to the plain unseeded parse — worst case the old
        // method-call divergence, never a crash.
        // Detect `# frozen_string_literal: true` on the ORIGINAL eval
        // source (the lambda-wrap shadows `source` below and would push
        // the magic comment off line 1 where the scanner can't see it).
        // The wrapped body's literals still compile into this eval's
        // proto range, so stamping the range freezes them regardless of
        // the wrap. rack Builder.parse_file evals frozen.ru this way.
        let fsl = crate::compiler::detect_frozen_string_literal(source);
        let (prog, compile_params, effective_seed, registered_source) =
            match Self::prepare_eval_body(source, &local_seed) {
                Ok(t) => t,
                Err(msg) => return Err(self.trap(RubyError::SyntaxError { msg })),
            };
        let source: &str = &registered_source;
        // Cap-aware compile: `compile_proto` interns method names,
        // locals, constants, and other symbols from the eval'd
        // source. Unlike top-level / require source (which is host-
        // loaded under embedder control), eval'd strings can be
        // dynamically constructed by Ruby code — `eval("def m#{i};
        // end")` in a loop would grow the interner past any
        // configured cap. Pre-check BEFORE registering the source
        // so a rejected eval doesn't leak a `(eval):N` source entry.
        // Post-check after compile removes the source entry on cap
        // failure for the same reason — best-effort: the interner
        // may briefly grow past the cap before the trap fires (we
        // don't roll back interns themselves).
        let cap_at_entry = self.max_symbols;
        if let Some(max) = cap_at_entry
            && self.interner.len() >= max {
            return Err(self.trap(RubyError::ResourceExhausted {
                msg: format!("interner exhausted before eval: {} symbols", max),
            }));
        }
        // Avoid clobbering a previously-registered source ONLY for
        // synthetic default labels: on collision, append an
        // incrementing `:N` suffix to the source-table key so the
        // prior entry is preserved for backtraces /
        // `Method#source_location`. User-supplied filenames
        // (`synthetic = false`) pass through unchanged so
        // `__FILE__` keeps returning the caller's name across
        // repeated evals — a `:N` suffix would leak into
        // observable metadata (`eval("__FILE__", nil, "foo.rb")`
        // called twice should both see "foo.rb", not "foo.rb:2",
        // and the same holds for the rare case where the caller
        // explicitly passes the literal default label as a
        // filename). The trade-off: explicit-filename collisions
        // clobber the source-table entry (matching CRuby's actual
        // behavior); the suffix dedupe applies only to the common
        // ephemeral case of repeated bare `eval(...)` /
        // `cls.class_eval(str)` calls where we ourselves chose
        // the default label.
        let mut effective_filename: String = filename.to_string();
        if synthetic && self.sources.contains_key(effective_filename.as_str()) {
            let mut n: u64 = 2;
            loop {
                let candidate = format!("{}:{}", filename, n);
                if !self.sources.contains_key(candidate.as_str()) {
                    effective_filename = candidate;
                    break;
                }
                n = n.saturating_add(1);
            }
        }
        let filename_rc: std::rc::Rc<str> = std::rc::Rc::from(effective_filename.as_str());
        let source_rc: std::rc::Rc<str> = std::rc::Rc::from(source);
        self.sources.insert(filename_rc.clone(), source_rc);
        let fsl_start = self.protos.len();
        // For `class_eval`/`module_eval` (class_ctx set), the eval'd code
        // runs with the receiver as its lexical cref, so bare constants
        // resolve through the receiver's namespace — `DC.module_eval("def
        // f; Element; end")` finds `RSS::Element` when DC is RSS::DC.
        // Seed the proto's class_path with the receiver's nesting (its
        // `::`-split effective name) so the const-chain walk includes the
        // outer scopes. Bare `eval` (no class_ctx) stays toplevel.
        // Surfaced by rss's dublincore.rb module_eval'd accessors.
        let eval_class_path: Vec<String> = class_ctx
            .as_ref()
            .and_then(|c| c.effective_name())
            .map(|n| n.split("::").map(|s| s.to_string()).collect())
            .unwrap_or_default();
        let entry = crate::compiler::compile_proto_at(
            "<eval>".into(), compile_params, &[prog], filename_rc.clone(),
            &mut self.protos, &mut self.interner, &mut self.cache_counter,
            eval_class_path,
        );
        if fsl {
            crate::compiler::mark_frozen_string_literal(&mut self.protos, fsl_start);
        }
        // Stamp the caller-supplied line offset over the whole eval
        // proto range so backtraces / source_location map onto the
        // caller's coordinate system. `prepare_eval_body` may wrap the
        // source in a single-line lambda prologue when `local_seed` is
        // non-empty (to make Prism resolve seeded names as locals);
        // that prologue shifts the body down one line, so compensate by
        // subtracting it from the base. (`mark_line_base` is a no-op
        // relative to the default when `line_base` is None.)
        if let Some(base) = line_base {
            crate::compiler::mark_line_base(&mut self.protos, fsl_start, base);
        }
        if let Some(enc) = source_encoding {
            crate::compiler::mark_source_encoding(&mut self.protos, fsl_start, enc);
        }
        if let Some(max) = cap_at_entry
            && self.interner.len() > max {
            // Don't leave the orphan source entry behind when we
            // refuse the eval — the compiled proto won't be
            // executed and nothing else will consult its source.
            self.sources.remove(&filename_rc);
            return Err(self.trap(RubyError::ResourceExhausted {
                msg: format!("eval grew interner past cap: {} symbols", max),
            }));
        }
        let cc = self.cache_counter.call as usize;
        let ivc = self.cache_counter.ivar as usize;
        self.ensure_ivar_caches(ivc);
        self.ensure_call_caches(cc);
        let depth_before = self.frames.len();
        let stack_before = self.stack.len();
        // `check_frames()` was already called at the top of
        // `eval_string` (before source registration / compile) so
        // the cap-rejected path leaves no VM state behind. The
        // frame stack hasn't grown since then — we haven't pushed
        // a frame yet — so we don't need a second check here.
        //
        // Class context (string-form class_eval): self = the
        // receiver class, is_class_body so `def` installs onto it,
        // and the class_stack entry mirrors what Op::DefClass
        // pushes for a literal `class X` body. Popped on EVERY
        // exit below (ok / trap / non-local return).
        let cls_depth_at_entry = self.class_stack.len();
        let vis_depth_at_entry = self.class_visibility_stack.len();
        if let Some(cls) = &class_ctx {
            self.class_stack.push(cls.clone());
            self.class_visibility_stack.push(crate::value::Visibility::Public);
        }
        // Seed the eval frame's locals from the Binding snapshot. The
        // seeded names were compiled as the leading params (slots
        // 0..local_seed.len()), so slot K holds local_seed[K]; any
        // locals the eval'd source itself introduces take later slots
        // and stay Nil.
        let mut seeded = super::vec_nil(self.protos[entry].n_locals as usize);
        for (i, (_, v)) in effective_seed.iter().enumerate() {
            if let Some(slot) = seeded.get_mut(i) {
                *slot = v.clone();
            }
        }
        // Top-level `self` for a require'd file / bare `eval` (no
        // self-override, no class context) is the `main` object, like
        // CRuby — so `self.extend Module` at a required file's top level
        // works (rake/dsl_definition.rb). instance_eval / class_eval
        // keep their explicit self / class context.
        let main_self = self.main_object();
        self.frames.push(super::Frame {
            proto_idx: entry,
            ip: 0,
            locals: crate::vm::Locals::Shared(std::rc::Rc::new(std::cell::RefCell::new(
                seeded
            ))),
            self_val: match (&self_override, &class_ctx) {
                (Some(s), _) => s.clone(),
                (None, Some(cls)) => Value::Class(cls.clone()),
                (None, None) => main_self,
            },
            base_sp: self.stack.len(),
            // NOT is_class_body even with a ctx: that flag drives
            // the class-body RETURN convention (pop class_stack,
            // discard the body value) — here the eval's last
            // expression IS the return value (CRuby:
            // `cls.class_eval("__FILE__")` → the string). def
            // installation only consults class_stack, which we
            // push/truncate around the run ourselves.
            is_class_body: false,
            swap_return: None,
            block_arg: None,
            defining_class: None,
            lexical_cvar_class: None,
            #[cfg(feature = "regex")] saved_last_match: None,
            is_block: false, is_lambda: false,
            n_given_positional: 0,
            kw_given_mask: 0,
            aux: None,
            pending_yield: false,
            block_writeback: None,
            dm_share: false,
            own_start: 0,
            outer_cell_start: 0,
            outer_cell: None,
            outer_rest: None,
            captured_yield_block: None,
        });
        // Same dispatch-loop shape as compile_and_run_source;
        // a non-local `return` defined INSIDE the eval'd string
        // should unwind locally, but a `return` escaping the
        // eval'd top level pops back into the caller's frame.
        loop {
            if let Err(t) = self.dispatch_until(depth_before) {
                // Restore-to-depth, not unconditional pop: the
                // normal frame-return path (is_class_body) pops the
                // ctx itself; only an abnormal exit leaves it.
                self.class_stack.truncate(cls_depth_at_entry);
                self.class_visibility_stack.truncate(vis_depth_at_entry);
                return Err(t);
            }
            if self.method_return.is_none() {
                break;
            }
            // `eval` deliberately keeps the legacy "walk blocks,
            // pop one method" unwind here — DO NOT mirror the
            // layer-4 lexical-owner walk that
            // `require_in_filescope` uses. CRuby's semantics for
            // `return` originating in eval'd top-level code are
            // "return from the method enclosing the eval call"
            // (RUBY_TAG_RETURN propagates past the eval boundary),
            // NOT "return from the eval'd <main>". A lexical-walk
            // here whose owner_rc points at the eval's <main>
            // locals would stop *at* eval's <main> and assign the
            // return value back to the eval-call's caller — the
            // wrong semantics. The legacy "pop one method"
            // followed by escape-to-outer-dispatch correctly
            // funnels return-from-eval through the enclosing
            // method. (code-review #285 round 2 #1 — adopted as
            // "no change" with documenting comment after we
            // verified the lexical walk gave the wrong answer
            // for `eval(\"outer_eval { return :b }\")`.)
            //
            // The dual-method-frame chain (`outer { lex { return } }`
            // entirely inside eval) is intentionally accepted to
            // mis-pop one frame in this path — a known Tier-1
            // divergence vs CRuby that ships separately from
            // layer #4 if it ever becomes load-bearing for a
            // real script.
            let val = self.take_method_return().unwrap();
            // `take_method_return` already cleared
            // `method_return_locals` paired with the value — no
            // dangling Rc to worry about on the escape branch
            // below. (code-review #285 round 2 #2 — the field-pair
            // invariant the helper enforces is what makes this
            // legacy path safe to leave alone.)
            while let Some(f) = self.frames.last() {
                if !f.is_block { break; }
                if self.frames.len() <= depth_before + 1 {
                    break;
                }
                let f = self.frames.pop().unwrap();
                self.stack.truncate(f.base_sp);
                if f.dm_share {
                    self.dm_share_depth = self.dm_share_depth.saturating_sub(1);
                }
                if f.is_class_body {
                    let _cls = self.class_stack.pop()
                        .expect("ICE: class_stack empty unwinding through class_eval (eval_string)");
                    self.class_visibility_stack.pop();
                    self.module_function_active_stack.pop();
                }
                self.release_frame_locals(f.locals);
            }
            if self.frames.len() <= depth_before + 1 {
                self.method_return = Some(val);
                self.sync_control_signals();
                break;
            }
            let f = self.frames.pop().unwrap();
            self.stack.truncate(f.base_sp);
            if f.dm_share {
                self.dm_share_depth = self.dm_share_depth.saturating_sub(1);
            }
            if f.is_class_body {
                let cls = self.class_stack.pop()
                    .expect("ICE: class_stack empty on method-return (eval_string)");
                self.class_visibility_stack.pop();
                self.module_function_active_stack.pop();
                self.stack.push(Value::Class(cls));
            } else if let Some(r) = f.swap_return {
                self.stack.push(r);
            } else {
                self.stack.push(val);
            }
            self.release_frame_locals(f.locals);
        }
        self.class_stack.truncate(cls_depth_at_entry);
        self.class_visibility_stack.truncate(vis_depth_at_entry);
        // Method-return escaping out of the eval — let outer
        // dispatch finish the unwind.
        if self.method_return.is_some() {
            self.suppress_call_result_push = true;
            return Ok(Value::Nil);
        }
        // Outer-rescue-unwound-past-us — stack already truncated
        // by the rescue handler; suppress the result push.
        if self.stack.len() != stack_before + 1 {
            self.suppress_call_result_push = true;
            return Ok(Value::Nil);
        }
        // Normal completion: the eval'd source's last expression
        // sits on top of the operand stack. Pop and return it
        // (CRuby semantics: `eval("1+2")` → 3).
        Ok(self.stack.pop().unwrap_or(Value::Nil))
    }

}

/// Top-level constant name(s) each stdlib require conventionally
/// exposes. After a stubbed `require 'X'` we install an empty
/// `Class` for each so `defined?(X)` resolves to "constant" and
/// `X.name` returns the right string. The shell carries no
/// methods; actual calls still fail with NoMethodError.
///
/// Names without an obvious top-level constant return an empty
/// slice: `digest/sha1` extends `Digest`; `English` installs
/// `$ERROR_INFO`-style aliases; `time` extends the `Time` class
/// which rubyrs doesn't model.
///
/// Note on Module vs Class: CRuby distinguishes `URI` /
/// `JSON` (Modules) from `Logger` (Class). rubyrs doesn't model
/// Module separately, so every stub is a Class. `is_a?(Module)`
/// returns true in CRuby for both shapes and matches rubyrs's
/// Class is-a-Module behaviour; `is_a?(Class)` diverges
/// (true in rubyrs for everything; false in CRuby for the
/// Module-shaped names). Documented divergence; fixtures
/// probe `defined?` and `.name` which agree.
///
/// Cfg-gated on `not(wasi)` to match the sole caller's gate
/// (the `"require" => match args { ... }` arm in
/// `builtin_call`, line ~547). Under `wasm32-wasip1
/// --no-default-features` the caller is cfg'd out, so this
/// function becomes dead code; the gate keeps the
/// `-D warnings` build green.
#[cfg(not(target_os = "wasi"))]
fn stdlib_constant_names(name: &str) -> &'static [(&'static str, bool)] {
    // Each entry is (constant_name, is_module). `true` for
    // names CRuby exposes as `Module` (URI / JSON / Base64 /
    // Forwardable / Singleton / FileUtils / Digest / YAML
    // / Math / SecureRandom / Open3 / Shellwords / FileTest
    // / CGI / Kernel-like utility namespaces); `false` for
    // names CRuby exposes as `Class` (Logger / Set / Pathname
    // / Tempfile / StringIO / Date / OpenStruct / Delegator /
    // OptionParser / BigDecimal / Monitor / ERB / WeakRef).
    match name {
        "uri" | "uri/generic" | "uri/common" => &[("URI", true)],
        "set" => &[("Set", false)],
        "logger" => &[("Logger", false)],
        "forwardable" => &[("Forwardable", true), ("SingleForwardable", true)],
        "singleton" => &[("Singleton", true)],
        "delegate" => &[("Delegator", false), ("SimpleDelegator", false)],
        "ostruct" => &[("OpenStruct", false)],
        "pathname" => &[("Pathname", false)],
        "stringio" => &[("StringIO", false)],
        "strscan" => &[("StringScanner", false)],
        "fileutils" => &[("FileUtils", true)],
        "digest" => &[("Digest", true)],
        "digest/md5" | "digest/sha1" | "digest/sha2" => &[],
        "base64" => &[("Base64", true)],
        "securerandom" => &[("SecureRandom", true)],
        "json" => &[("JSON", true)],
        "yaml" => &[("YAML", true)],
        // CRuby exposes `Psych` as a Module (YAML is literally Psych).
        // The require hook aliases the two constants either way; this
        // makes a bare `require "psych"` materialise the shell too.
        "psych" => &[("Psych", true)],
        "date" => &[("Date", false), ("DateTime", false)],
        // CRuby's lib/time.rb does `require 'date'` internally, so
        // `require "time"` makes Date / DateTime resolvable too.
        // Discovery: P3 Jekyll spike — safe_yaml/parse/date.rb does
        // `require 'time'` then references bare `DateTime`.
        "time" => &[("Date", false), ("DateTime", false)],
        "csv" => &[("CSV", false)],
        "english" | "English" => &[],
        "bigdecimal" => &[("BigDecimal", false)],
        "monitor" => &[("Monitor", false), ("MonitorMixin", true)],
        "mutex_m" => &[("Mutex_m", true)],
        "benchmark" => &[("Benchmark", true)],
        "erb" => &[("ERB", false)],
        "open3" => &[("Open3", true)],
        "shellwords" => &[("Shellwords", true)],
        "weakref" => &[("WeakRef", false)],
        "cgi" | "cgi/util" | "cgi/escape" | "cgi/cookie" => &[("CGI", false)],
        // `ipaddr`: Sinatra 4 + rack-protection 4 `require 'ipaddr'`
        // at module-load time. Class-body usage is constant-check
        // shape (`when IPAddr`, `rescue IPAddr::InvalidAddressError`)
        // which doesn't actually call any IPAddr methods at load.
        // `IPAddr.new(...)` calls are wrapped in lambdas/Procs that
        // run later — bare constant shell suffices to clear the load.
        "ipaddr" => &[("IPAddr", false)],
        "openssl" => &[("OpenSSL", true)],
        "zlib" => &[("Zlib", true)],
        "fiber" => &[("Fiber", false)],
        "rbconfig" => &[("RbConfig", true)],
        _ => &[],
    }
}
impl Vm {
    /// Marshal-dumpability probe (see `__rubyrs_marshal_stash`).
    /// Walks the value graph and returns Err(message) on the first
    /// CRuby-rejected shape. Cycle-safe via a visited set; the walk
    /// is depth-capped defensively (a graph deeper than this is
    /// pathological and we'd rather accept than stack-overflow —
    /// accepting only weakens the probe, never corrupts).
    pub(crate) fn marshal_dumpable(&self, root: &Value) -> Result<(), String> {
        use crate::heap::HeapObj;
        let mut seen: Vec<u32> = Vec::new();
        let mut stack: Vec<Value> = vec![root.clone()];
        let mut budget = 10_000usize;
        while let Some(v) = stack.pop() {
            if budget == 0 {
                return Ok(());
            }
            budget -= 1;
            match &v {
                Value::Block(_) | Value::CurriedProc(_) => {
                    return Err("no _dump_data is defined for class Proc".into());
                }
                Value::BoundMethod(_) => {
                    return Err("no _dump_data is defined for class Method".into());
                }
                Value::UnboundMethod(_) => {
                    return Err("no _dump_data is defined for class UnboundMethod".into());
                }
                Value::Object(id) => {
                    if seen.contains(&id.0) {
                        continue;
                    }
                    seen.push(id.0);
                    match self.heap.get(*id) {
                        HeapObj::Instance(inst) => {
                            // Binding is CRuby's canonical
                            // un-marshalable: minitest's neuter
                            // chain is triggered by exceptions
                            // carrying one in an ivar.
                            if inst.class.name == "Binding" {
                                return Err("no _dump_data is defined for class Binding".into());
                            }
                            if inst.singleton_class.is_some() {
                                return Err(format!(
                                    "singleton can't be dumped (instance of {})",
                                    inst.class.name
                                ));
                            }
                            // Use the EFFECTIVE name so a const-assigned
                            // class (`S = Struct.new(...)` / `Foo =
                            // Class.new`) — whose raw `name` is empty but
                            // whose lazily-stamped `assigned_name` is set —
                            // is NOT treated as anonymous (it dumps fine
                            // via the `S`/`o` tags). A truly anonymous
                            // instance still raises.
                            let ename = inst.class.effective_name().unwrap_or_default();
                            if ename.is_empty() || ename.starts_with("#<") {
                                return Err("can't dump anonymous class".into());
                            }
                            // A class with its own marshal hooks
                            // (`marshal_dump` / `_dump`) REPLACES its
                            // ivar graph with the hook payload — don't
                            // reject (or descend) based on raw ivars.
                            // rubocop's Offense#marshal_dump drops
                            // @corrector, whose TreeRewriter graph
                            // holds Procs; the raw-ivar walk would
                            // veto a dump CRuby accepts. (`get_id`
                            // not `intern`: a name nothing interned
                            // is a name nothing defines.)
                            let has_hook = ["marshal_dump", "_dump"].iter().any(|n| {
                                self.interner
                                    .get_id(n)
                                    .is_some_and(|sid| {
                                        self.lookup_method_uncached(&inst.class, sid).is_some()
                                    })
                            });
                            if !has_hook {
                                for iv in inst.ivars.values() {
                                    stack.push(iv.clone());
                                }
                            }
                        }
                        // IO-ish / opaque host shapes can't serialize.
                        _ => return Err("no _dump_data is defined for this object".into()),
                    }
                }
                Value::Array(id) => {
                    if seen.contains(&id.0) {
                        continue;
                    }
                    seen.push(id.0);
                    match self.heap.get(*id) {
                        crate::heap::HeapObj::Array(a) => {
                            for e in a.elems.iter() {
                                stack.push(e.clone());
                            }
                            for iv in a.ivars.values() {
                                stack.push(iv.clone());
                            }
                        }
                        _ => return Err("no _dump_data is defined for this object".into()),
                    }
                }
                Value::Hash(id) => {
                    if seen.contains(&id.0) {
                        continue;
                    }
                    seen.push(id.0);
                    match self.heap.get(*id) {
                        crate::heap::HeapObj::Hash(h) => {
                            if h.default_block().is_some() {
                                return Err("can't dump hash with default proc".into());
                            }
                            for (k, val) in h.pairs.iter() {
                                stack.push(k.clone());
                                stack.push(val.clone());
                            }
                            if let Some(iv) = h.ivars() {
                                for v in iv.values() {
                                    stack.push(v.clone());
                                }
                            }
                        }
                        _ => return Err("no _dump_data is defined for this object".into()),
                    }
                }
                Value::Range(id) => {
                    if seen.contains(&id.0) {
                        continue;
                    }
                    seen.push(id.0);
                    let r = self.heap.range(*id);
                    stack.push(r.begin.clone());
                    stack.push(r.end.clone());
                }
                _ => {}
            }
        }
        Ok(())
    }
}


/// Known stdlib-shaped require names that rubyrs Tier 1 stubs to
/// `true` rather than load. Whitelist is conservative: only the
/// names script authors typically `require` for feature-detection
/// or as no-op dependencies of larger files. Anything not in this
/// set falls through to cext (or "cannot find" if cext is off).
///
/// Aligns with ADR 0017 (stdlib is Tier 3; this is the Tier 1
/// lenient-mode bridge that lets gem helpers load). Scripts that
/// actually USE the stdlib's API (`URI.parse`, `Logger.new`,
/// `JSON.parse`, ...) get NameError / NoMethodError at the call
/// site — sharper "feature absent" surface than a failed require.
///
/// See the gate note on `stdlib_constant_names` above — same
/// reasoning, same cfg.
#[cfg(not(target_os = "wasi"))]
fn is_stdlib_stub_name(name: &str) -> bool {
    // `bcrypt_ext`: the bcrypt gem's C extension, provided by the
    // `_bcrypt` battery (BCrypt::Engine is defined at startup). The
    // require just needs to succeed; routing it here bypasses the
    // cext/.bundle path entirely.
    #[cfg(feature = "_bcrypt")]
    if name == "bcrypt_ext" {
        return true;
    }
    // `oj/oj`: the oj gem's C extension, provided by the `_oj` battery
    // (the `Oj` module is defined at startup). Succeed the require so the
    // gem's `require "oj/oj"` resolves instead of LoadError-ing on the
    // .bundle.
    #[cfg(feature = "_oj")]
    if name == "oj/oj" {
        return true;
    }
    matches!(
        name,
        "uri" | "uri/generic" | "uri/common"
        | "set" | "logger" | "forwardable"
        | "singleton" | "delegate" | "ostruct"
        | "pathname" | "tempfile" | "tmpdir" | "stringio" | "strscan" | "fileutils"
        | "digest" | "digest/md5" | "digest/sha1" | "digest/sha2"
        | "base64" | "securerandom"
        | "json" | "yaml" | "date" | "time" | "csv"
        // safe_yaml: rubyrs ships a focused YAML loader (yaml.rb) and
        // routes safe_yaml's load API to it, bypassing the real gem's
        // Psych::Handler internals. See `is_blessed_reimpl_name`.
        | "safe_yaml" | "safe_yaml/load"
        // jekyll-sass-converter: shim routes SCSS→CSS to the grass
        // `sass` battery instead of native sass-embedded.
        | "jekyll-sass-converter"
        | "english" | "English"
        // `optparse`: vendored real parser (stdlib_vendor/optparse.rb)
        | "optparse"
        | "bigdecimal" | "bigdecimal/util" | "monitor" | "mutex_m" | "benchmark" | "erb" | "erubi"
        // `sinatra` / `sinatra/base` / `sinatra/version`: rubyrs's blessed
        // in-tree micro-Sinatra (stdlib_vendor/sinatra_base.rb). Lets a real
        // Sinatra app + the sinatra-* extension gems load with zero code
        // change (`require "sinatra"`), instead of pulling the real gem.
        | "sinatra" | "sinatra/base" | "sinatra/version" | "sinatra/reloader"
        // `pp`: Kernel#pp is native; the vendored pp.rb adds
        // Object#pretty_inspect + the PP module. faraday's logging
        // formatter `require 'pp'` for `Hash#pretty_inspect`.
        | "pp"
        // `etc`: vendored Etc.nprocessors subset (stdlib_vendor/etc.rb)
        // — minitest requires it unconditionally at load.
        | "etc"
        // `timeout`: lenient shell (rack's spec_utils requires it
        // for one Timeout::Error assertion; Timeout.timeout itself
        // needs real preemption — out of the single-threaded
        // model). The vendored stub defines the constants so the
        // require + rescue-class references resolve.
        | "timeout"
        // `io/wait` (IO#wait_readable / #wait_writable) and `resolv`
        // (DNS resolver): net/protocol requires both at load time, but
        // the `_socket` battery's blocking TCPSocket veneer provides the
        // readiness methods directly and does its own host-side DNS — so
        // these are lenient no-op shells just to satisfy the require.
        | "io/wait" | "resolv"
        // `open-uri`: extends Kernel#open / URI.open with HTTP(S) fetch
        // (built on net/http). Gems require it at load but only call it
        // from request-time methods — rss's parser.rb requires it for
        // `URI.open(feed_url)`, never reached by parsing an in-memory
        // String. Lenient shell satisfies the require; an actual
        // `URI.open(url)` raises NoMethodError (feature-absent contract).
        | "open-uri"
        // `io/console`: required at load time by the `console` gem's
        // terminal output (samovar → bridgetown CLI). Its only real
        // method use (`IO#winsize`) is on the TTY-only xterm path, never
        // reached for non-tty/piped output — so a lenient no-op shell
        // satisfies the require; `IO#winsize`/`#raw` etc. raise
        // NoMethodError (feature-absent contract) if ever called.
        | "io/console"
        | "open3" | "shellwords" | "weakref"
        | "cgi" | "cgi/util" | "cgi/escape" | "cgi/cookie"
        | "ipaddr"
        // `openssl`: rack-session's cookie / encryptor `require
        // 'openssl'` at module-load time but only call
        // `OpenSSL::Cipher.new` / `OpenSSL::HMAC.digest` from
        // inside request-time methods (cookie signing). The
        // lenient stub materialises the `OpenSSL` module so the
        // require succeeds; real crypto stays behind the Tier-3
        // `_openssl` battery (rustls — ADR 0019 Part E). Calls
        // into the stub raise NoMethodError (feature-absent
        // surface), so an app that actually signs cookies fails
        // loudly rather than silently mis-signing.
        | "openssl"
        // `zlib`: rack-session's cookie store `require 'zlib'` at
        // load time for optional payload compression
        // (`Zlib::Deflate` / `Inflate`), called only at request
        // time. Same lenient-stub rationale as `openssl` above —
        // real deflate/inflate would be a future Tier-3 battery.
        | "zlib"
        // `fiber`: Rack 3 `rack/builder.rb` requires this. rubyrs
        // has no Fiber primitive (Tier 1 = single-threaded,
        // single-fiber); the require is a no-op stub. Downstream
        // `Fiber.new { ... }` raises NoMethodError at the bare
        // constant-shell surface — matches CRuby's "feature
        // absent" contract.
        | "fiber"
        // `rbconfig`: Ruby's build configuration module. Some
        // gems in the Sinatra/Rack chain require it to detect
        // host_os / platform. rubyrs has no build-config to
        // expose; the require is a no-op stub. `RbConfig::CONFIG`
        // accesses raise NoMethodError on the bare module shell
        // — matches CRuby's "feature absent" contract.
        | "rbconfig"
        // `rubygems` no-op: rubyrs preloads a minimal `Gem::Version`
        // shim in the preamble (see preamble/gem.rb). The stub
        // lets explicit `require 'rubygems'` in user code / test
        // fixtures succeed so they can opt into the full RubyGems
        // surface on CRuby's side.
        | "rubygems"
        | "rack"
        // `tilt`: Sinatra 4 requires it at module-load time. The
        // shim (always_on_stub_extras) installs a `Tilt` module
        // with a no-op `default_mapping` good enough for hello-
        // world Sinatra apps that don't actually render views.
        // Real template rendering remains out of scope per
        // ADR 0017 — `Tilt[engine]` etc. raise NoMethodError.
        | "tilt"
        // ActiveSupport-lite — menu item 3. Three common require
        // shapes, all routed to the same canon under `stdlib`
        // (see `stdlib_vendor::stdlib_vendor_source`). Default
        // Tier-1 build keeps the lenient stub (constant exists,
        // methods raise) per the existing whitelist's contract.
        | "active_support"
        | "active_support/all"
        | "active_support/core_ext"
        // `_sqlite` battery — ADR 0027 / menu item 4. Per ADR
        // 0019 Rule 8 the load form is `rubyrs/sqlite` (NOT bare
        // `sqlite3`) so MRI's gem stays loadable independently
        // when Tier-4 compat lands. The constants get installed
        // by the preamble pipeline at Runtime construction;
        // the require itself is a no-op stub confirming "yes,
        // the battery is in this build."
        | "rubyrs/sqlite"
    )
}

/// Names whose rubyrs vendored implementation must take precedence
/// over an on-disk gem of the same name (ADR 0026 blessed reimpl).
/// The real gems can't run on rubyrs — `safe_yaml` subclasses
/// `Psych::Handler` — so we route the require to the vendored
/// stub/loader even when the gem is installed and on `$LOAD_PATH`.
/// Only used from the non-wasi require path, so gated to match.
#[cfg(not(target_os = "wasi"))]
fn is_blessed_reimpl_name(name: &str) -> bool {
    matches!(
        name,
        "safe_yaml" | "safe_yaml/load" | "jekyll-sass-converter"
        // `digest` is a C extension (OpenSSL-backed) in CRuby; it
        // cannot be hosted. Route every `require "digest"` /
        // `"digest/sha2"` / ... to the native `RubyrsDigest`-backed
        // veneer even when CRuby's own `digest.rb` is on `$LOAD_PATH`
        // (it would otherwise `require "digest.so"` and fail).
        | "digest" | "digest/sha2" | "digest/sha1" | "digest/md5"
    )
}

/// ASCII-lowercase name → "is this the preamble-defined core
/// class for that name?" Used by
/// `Vm::require_satisfied_by_existing_constant` to block
/// `require "string"` / `require "array"` from silently
/// succeeding against the preamble's `class String` /
/// `class Array`. Anything an embedder or user script defines
/// later isn't on this list and still triggers the lenient
/// fallback.
///
/// Sources from `crates/rubyrs/src/lib.rs` (~750-1100): every
/// `class Foo` / `module Foo` in the preamble. Keep in sync if
/// new core classes land. Normalized to lowercase to share
/// shape with the case-insensitive walk's compare so
/// `require "OBJECT"` is rejected too.
#[cfg(not(target_os = "wasi"))]
fn is_core_preamble_class_name(lowered_first_seg: &str) -> bool {
    matches!(
        lowered_first_seg.to_ascii_lowercase().as_str(),
        // value classes
        "object" | "integer" | "float" | "string" | "symbol"
        | "array" | "hash" | "range" | "trueclass" | "falseclass"
        | "nilclass" | "proc" | "method" | "unboundmethod"
        | "module" | "class" | "file" | "mutex" | "kernel"
        | "matchdata" | "comparable" | "enumerable"
        // exception hierarchy
        | "exception" | "standarderror" | "runtimeerror"
        | "nomethoderror" | "argumenterror" | "typeerror"
        | "nameerror" | "scripterror" | "notimplementederror"
        | "indexerror" | "keyerror" | "zerodivisionerror"
        | "rangeerror" | "localjumperror" | "frozenerror"
        | "resourceexhausted"
    )
}

/// Convert a snake_case Ruby file/require token to CamelCase
/// using Rubygems / Bundler's standard heuristic: split on `_`,
/// capitalize each part, concat. `rack` → `Rack`, `active_record`
/// → `ActiveRecord`, `my_lib_v2` → `MyLibV2`. Empty input yields
/// empty output (caller filters those before invoking).
///
/// Gated to non-wasi: the only caller
/// (`Vm::require_satisfied_by_existing_constant`) is itself
/// non-wasi, so under CI's `RUSTFLAGS=-D warnings` this helper
/// would otherwise trip a dead-code warning on wasm32-wasip1.
#[cfg(not(target_os = "wasi"))]
fn snake_to_camel_case(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for part in input.split('_') {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            for c in first.to_uppercase() {
                out.push(c);
            }
            out.extend(chars);
        }
    }
    out
}

impl Vm {
    /// `Kernel#Integer(x [, base] [, exception: true/false])`.
    ///
    /// The String path is the shared strict `str2int` scanner —
    /// see that module's docs for the probed CRuby 3.4 semantics
    /// (prefixes, underscores, whitespace, negative bases, BigInt
    /// promotion). This wrapper owns the CRuby argument protocol,
    /// in CRuby's check order:
    ///
    ///   1. keywords: `exception:` must be literal true/false
    ///      (`expected true or false as exception: <inspect>`) —
    ///      raises even when the value would parse fine, and is
    ///      NEVER suppressed;
    ///   2. arity (1..2 positionals);
    ///   3. the conversion itself — its ArgumentError / TypeError /
    ///      FloatDomainError all become `nil` under
    ///      `exception: false`, EXCEPT the invalid-radix
    ///      ArgumentError, which the scan raises lazily (CRuby's
    ///      bignum.c order — a prefix-resolved base skips
    ///      validation; `Integer("0x10", -99)` parses) and which is
    ///      never suppressed (probed: `Integer("10", 99,
    ///      exception: false)` still raises).
    ///
    /// rubyrs flattens kwargs into a trailing positional Hash, but
    /// `Vm::trailing_hash_positional` (set by plain `Op::Call`,
    /// cleared by the `CallKw`/splat/`super`/block routes) records
    /// HOW that hash was passed — the same signal the user-method
    /// keyword binder consumes at bind time (dispatch.rs). Gating
    /// the peel on it keeps `Integer("42", {exception: false})`
    /// (literal brace hash → positional → radix TypeError, as
    /// CRuby) distinct from `Integer("42", exception: false)` (real
    /// keywords). Once kwargs syntax is established, non-`exception`
    /// keys raise CRuby's `unknown keyword: <key.inspect>` (probed:
    /// Symbol AND non-Symbol keys — `Integer("42", **{"a" => 1})` →
    /// `unknown keyword: "a"`). Read-only (no `mem::take`): the
    /// builtin completes the call here, and the plain-`Op::Call`
    /// arm resets the flag itself after `do_call` returns — taking
    /// it would blind a nested re-dispatch (`send(:Integer, ...)`)
    /// that relies on the outer call's flag. Known gaps kept as-is
    /// (out of this change's scope): no `to_int`/`to_str`/`to_i`
    /// coercion for arbitrary objects (CRuby coerces; we
    /// TypeError), and big finite Floats saturate at i64 rather
    /// than promoting.
    fn kernel_integer(&mut self, args: &[Value]) -> Result<Value, Trap> {
        use crate::vm::str2int::{self, ParsedInt};
        // ---- 1. keywords (only when passed WITH kwargs syntax) ----
        let mut positional = args;
        let mut exception = true;
        if !self.trailing_hash_positional
            && let Some(Value::Hash(hid)) = args.last()
        {
            let pairs = self.heap.hash(*hid).to_vec();
            if !pairs.is_empty() {
                positional = &args[..args.len() - 1];
                // CRuby order: unknown keywords first, then the
                // `exception:` true/false type check. Last-wins on
                // duplicate keys, matching Hash construction order.
                let mut unknown: Vec<String> = Vec::new();
                let mut exc_val: Option<Value> = None;
                for (k, v) in &pairs {
                    match k {
                        Value::Sym(s) if &**self.interner.resolve(*s) == "exception" => {
                            exc_val = Some(v.clone());
                        }
                        other => {
                            unknown.push(other.to_inspect(&self.heap, &self.interner));
                        }
                    }
                }
                if !unknown.is_empty() {
                    let msg = if unknown.len() == 1 {
                        format!("unknown keyword: {}", unknown[0])
                    } else {
                        format!("unknown keywords: {}", unknown.join(", "))
                    };
                    return Err(self.trap(RubyError::ArgumentError { msg }));
                }
                match exc_val {
                    Some(Value::Bool(b)) => exception = b,
                    Some(other) => {
                        return Err(self.trap(RubyError::ArgumentError {
                            msg: format!(
                                "expected true or false as exception: {}",
                                other.to_inspect(&self.heap, &self.interner),
                            ),
                        }));
                    }
                    None => {}
                }
            }
        }
        // ---- 2. arity ----
        if positional.is_empty() || positional.len() > 2 {
            return Err(self.trap(RubyError::ArgumentError {
                msg: format!(
                    "wrong number of arguments (given {}, expected 1..2)",
                    positional.len(),
                ),
            }));
        }
        // ---- 3. radix shape (validation itself is LAZY — inside
        // the scan, per CRuby's bignum.c order; see str2int) ----
        let radix_arg: Option<i64> = match positional.get(1) {
            None => None,
            Some(Value::Int(r)) => Some(*r),
            Some(other) => {
                return Err(self.trap(RubyError::TypeError {
                    // CRuby num2long shape: nil gets "from nil to integer"
                    // (probed vs 3.4.1); others value-word "of X into Integer".
                    msg: other.num2int_conv_msg(),
                }));
            }
        };
        // ---- 4. conversion ----
        let err = match (&positional[0], radix_arg) {
            (Value::Int(n), None) => return Ok(Value::Int(*n)),
            // Integer identity holds for the Bignum span too:
            // `Integer(2**100)` is the value itself, not a
            // TypeError (this is also what makes
            // `Integer(Integer(big_str))` idempotent).
            #[cfg(feature = "bignum")]
            (v @ Value::BigInt(_), None) => return Ok(v.clone()),
            (Value::Float(f), None) => {
                // CRuby raises FloatDomainError (a RangeError
                // subclass) for NaN / ±Infinity here, matching
                // `Float#to_i`'s shape — same message label so
                // `Integer(Float::NAN)` and `Float::NAN.to_i`
                // emit the same exception class, not divergent
                // ones.
                if f.is_finite() {
                    return Ok(Value::Int(*f as i64));
                }
                RubyError::FloatDomainError {
                    msg: crate::vm::numeric::float_domain_label(*f).to_string(),
                }
            }
            (Value::Str(s), r) => {
                match str2int::strict(&s.borrow(), r.unwrap_or(0)) {
                    Ok(Some(ParsedInt::Small(n))) => return Ok(Value::Int(n)),
                    #[cfg(feature = "bignum")]
                    Ok(Some(ParsedInt::Big(b))) => return self.bigint_to_value(b),
                    // Invalid radix — probed: NOT suppressed by
                    // `exception: false` (unlike the invalid-value
                    // ArgumentError below).
                    Err(e) => return Err(self.trap(e)),
                    // CRuby's message embeds the receiver's INSPECT
                    // form (`Integer("4\n2")` → `... "4\n2"` with a
                    // literal backslash-n), not the raw bytes.
                    Ok(None) => RubyError::ArgumentError {
                        msg: format!(
                            "invalid value for Integer(): {}",
                            crate::heap::rstr_inspect(s),
                        ),
                    },
                }
            }
            (Value::Nil, None) => RubyError::TypeError {
                msg: "can't convert nil into Integer".into(),
            },
            (other, None) => RubyError::TypeError {
                msg: format!("can't convert {} into Integer", other.conv_type_name()),
            },
            // CRuby's exact message for `Integer(non_string, radix)`
            // is `"base specified for non string value"` — an
            // ArgumentError, NOT a TypeError, because the radix only
            // makes sense paired with a String to parse.
            (_other, Some(_)) => RubyError::ArgumentError {
                msg: "base specified for non string value".into(),
            },
        };
        if exception {
            Err(self.trap(err))
        } else {
            Ok(Value::Nil)
        }
    }
}

/// ADR 0025 Phase 0.5b: shared exit-status parser used by
/// `exit` and `exit!`. Accepts the CRuby shapes:
/// - no args   → 0
/// - true      → 0
/// - false     → 1
/// - nil       → 0
/// - Integer   → as-is (truncated to i32)
/// - anything else → TypeError
///
/// Returns `Result<i32, Option<Result<Value, Trap>>>` — the outer
/// `Option<Result<Value, Trap>>` matches `builtin_call`'s return
/// type so the caller can early-return via `?`.
fn parse_exit_status(args: &[Value]) -> Result<i32, Option<Result<Value, Trap>>> {
    match args {
        [] => Ok(0),
        [Value::Bool(true)] => Ok(0),
        [Value::Bool(false)] => Ok(1),
        [Value::Nil] => Ok(0),
        [Value::Int(n)] => Ok(*n as i32),
        [other] => Err(Some(Err(Trap {
            err: RubyError::TypeError {
                msg: other.num2int_conv_msg(),
            },
            backtrace: vec![],
        }))),
        _ => Err(Some(Err(Trap {
            err: RubyError::ArgumentError {
                msg: format!(
                    "wrong number of arguments (given {}, expected 0..1)",
                    args.len(),
                ),
            },
            backtrace: vec![],
        }))),
    }
}

/// ADR 0025 Phase 0.5b: construct a SystemExit instance with the
/// given status + message and route through the existing
/// `unwind_with_exception` machinery. Shared by `Kernel#exit` and
/// `Kernel#abort`.
///
/// Returns the `builtin_call`-shaped tuple. If unwind finds a
/// rescue handler, `suppress_call_result_push` is set so the
/// dispatch loop doesn't push a spurious Nil over the rescue
/// binding. If no handler, the trap propagates to the embedder
/// as `RubyError::Uncaught { class_name: "SystemExit", .. }`.
impl Vm {
    /// Fork-child status from a block/handler trap: an uncaught
    /// SystemExit carries its status in the `$!` instance's
    /// `@status` (the unwind already stamped `$!`); anything else
    /// prints the trap like a dying script and exits 1.
    #[cfg(all(unix, not(target_os = "wasi")))]
    fn fork_child_status_from_trap(&mut self, t: &Trap) -> i32 {
        if let RubyError::Uncaught { class_name, .. } = &t.err
            && class_name == "SystemExit"
        {
            if let Some(Value::Object(id)) = &self.last_uncaught_exception
                && let crate::heap::HeapObj::Instance(inst) = self.heap.get(*id)
            {
                let status_sym = self.interner.intern("@status");
                if let Some(Value::Int(n)) = inst.ivar_get(status_sym) {
                    return *n as i32;
                }
            }
            return 0;
        }
        eprintln!("rubyrs (fork child): {:?}", t.err);
        1
    }
}

/// Marshal 4.8 load-only reader (common-tag subset — see the
/// `__rubyrs_marshal_load_binary` arm). Object/symbol link tables
/// follow CRuby's registration order: every linkable object
/// registers BEFORE its children parse.
struct MarshalReader<'a> {
    b: &'a [u8],
    pos: usize,
    symbols: Vec<crate::intern::SymId>,
    objects: Vec<Value>,
    /// A trap raised by a reentrant `_load` / `marshal_load` hook —
    /// stashed so the load entry point can PROPAGATE the user exception
    /// rather than collapse it into the generic TypeError that a
    /// `Result<_, String>` error would become.
    trap: Option<Trap>,
    /// Set by the `C :Hash` wrapper (CRuby's compare_by_identity Hash
    /// encoding) so the inner `{`/`}` arm can flag the fresh Hash BEFORE
    /// inserting its pairs — identity-duplicate keys must NOT collapse
    /// through the eql?-aware insert.
    next_hash_by_identity: bool,
}

impl MarshalReader<'_> {
    fn byte(&mut self) -> Result<u8, String> {
        let c = *self.b.get(self.pos).ok_or("marshal data too short")?;
        self.pos += 1;
        Ok(c)
    }

    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        let end = self.pos.checked_add(n).ok_or("marshal length overflow")?;
        if end > self.b.len() {
            return Err("marshal data too short".into());
        }
        let s = &self.b[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    /// Marshal's variable-length long: 0 → 0; |c| in 1..=4 → that
    /// many little-endian payload bytes (sign-extended when c<0);
    /// otherwise the value is folded into the tag byte (c-5 / c+5).
    fn long(&mut self) -> Result<i64, String> {
        let c = self.byte()? as i8;
        Ok(match c {
            0 => 0,
            1..=4 => {
                let mut v: i64 = 0;
                for (i, &byte) in self.take(c as usize)?.iter().enumerate() {
                    v |= (byte as i64) << (8 * i);
                }
                v
            }
            -4..=-1 => {
                let n = (-c) as usize;
                let mut v: i64 = -1;
                for (i, &byte) in self.take(n)?.iter().enumerate() {
                    v &= !(0xff_i64 << (8 * i));
                    v |= (byte as i64) << (8 * i);
                }
                v
            }
            5..=127 => (c as i64) - 5,
            _ => (c as i64) + 5,
        })
    }

    /// Resolve a class-name symbol (from an `o`/`S`/`C`/`u`/`U` tag) to
    /// its `Class`. A `Struct.new` / `Class.new` class is anonymous to
    /// `vm.classes` but lives in the constants table under its assigned
    /// name, so try constants first.
    fn marshal_lookup_class(
        &self,
        vm: &mut Vm,
        class_val: &Value,
    ) -> Result<std::rc::Rc<crate::value::Class>, String> {
        let cname = match class_val {
            Value::Sym(s) => vm.interner.resolve(*s).to_string(),
            _ => return Err("class name must be a symbol".into()),
        };
        Self::marshal_resolve_cname(vm, &cname)
    }

    /// Resolve a Marshal class name to its Class, firing a pending autoload if
    /// the class isn't loaded. CRuby triggers autoloads during Marshal.load;
    /// zeitwerk's reload re-arms class autoloads, then Marshal.load
    /// re-materialises the dumped instances. Shared by marshal_lookup_class and
    /// the inline object/struct/data readers below.
    fn marshal_resolve_cname(vm: &mut Vm, cname: &str) -> Result<std::rc::Rc<crate::value::Class>, String> {
        let cid = vm.interner.intern(cname);
        if let Some(Value::Class(c)) = vm.constants.get(&cid).cloned() {
            return Ok(c);
        }
        if let Some(c) = vm.classes.get(&cid).cloned() {
            return Ok(c);
        }
        #[cfg(not(target_os = "wasi"))]
        if vm
            .fire_pending_autoload(cname)
            .map_err(|_| format!("undefined class/module {cname}"))?
        {
            if let Some(Value::Class(c)) = vm.constants.get(&cid).cloned() {
                return Ok(c);
            }
            if let Some(c) = vm.classes.get(&cid).cloned() {
                return Ok(c);
            }
        }
        Err(format!("undefined class/module {cname}"))
    }

    fn read_value(&mut self, vm: &mut Vm) -> Result<Value, String> {
        let tag = self.byte()?;
        match tag {
            b'0' => Ok(Value::Nil),
            b'T' => Ok(Value::Bool(true)),
            b'F' => Ok(Value::Bool(false)),
            b'i' => Ok(Value::Int(self.long()?)),
            // Bignum: sign byte, length in 16-bit words, magnitude
            // little-endian. Demotes to Int when it fits in i64
            // (rubyrs's BigInt invariant); registers in the object
            // link table either way (CRuby registers Bignums).
            b'l' => {
                let sign = self.byte()?;
                let words = self.long()?;
                let raw = self
                    .take(usize::try_from(words).map_err(|_| "bad bignum length".to_string())? * 2)?
                    .to_vec();
                #[cfg(feature = "bignum")]
                {
                    let s = if sign == b'-' {
                        num_bigint::Sign::Minus
                    } else {
                        num_bigint::Sign::Plus
                    };
                    let big = num_bigint::BigInt::from_bytes_le(s, &raw);
                    let v = vm
                        .bigint_to_value(big)
                        .map_err(|_| "allocation limit".to_string())?;
                    if let Value::BigInt(_) = &v {
                        vm.pinned.push(v.clone());
                    }
                    self.objects.push(v.clone());
                    Ok(v)
                }
                #[cfg(not(feature = "bignum"))]
                {
                    // No BigInt in this build: accept magnitudes that
                    // still fit i64 (the writer's own out-of-32-bit Int
                    // form arrives as `l`), reject genuinely big ones.
                    if raw.len() > 16 {
                        return Err("Bignum is not supported in this build (enable bignum)".into());
                    }
                    let mut mag: u128 = 0;
                    for (i, b) in raw.iter().enumerate() {
                        mag |= (*b as u128) << (8 * i);
                    }
                    let signed: i128 = if sign == b'-' { -(mag as i128) } else { mag as i128 };
                    let v = i64::try_from(signed)
                        .map(Value::Int)
                        .map_err(|_| "Bignum is not supported in this build (enable bignum)".to_string())?;
                    self.objects.push(v.clone());
                    Ok(v)
                }
            }
            b'f' => {
                let n = self.long()?;
                let raw = self.take(n.max(0) as usize)?;
                let txt = std::str::from_utf8(raw).map_err(|_| "bad float text".to_string())?;
                let f = match txt {
                    "inf" => f64::INFINITY,
                    "-inf" => f64::NEG_INFINITY,
                    "nan" => f64::NAN,
                    _ => txt.parse::<f64>().map_err(|_| "bad float text".to_string())?,
                };
                let v = Value::Float(f);
                self.objects.push(v.clone());
                Ok(v)
            }
            b'"' => {
                let n = self.long()?;
                let raw = self.take(n.max(0) as usize)?.to_vec();
                let v = match String::from_utf8(raw) {
                    Ok(s) => Value::new_str(s),
                    Err(e) => Value::new_str_bytes_binary(e.into_bytes()),
                };
                self.objects.push(v.clone());
                Ok(v)
            }
            b':' => {
                let n = self.long()?;
                let raw = self.take(n.max(0) as usize)?;
                let name = std::str::from_utf8(raw).map_err(|_| "bad symbol text".to_string())?;
                let sid = vm.interner.intern(name);
                self.symbols.push(sid);
                Ok(Value::Sym(sid))
            }
            b';' => {
                let idx = self.long()?;
                let sid = self
                    .symbols
                    .get(usize::try_from(idx).map_err(|_| "bad symlink".to_string())?)
                    .ok_or("bad symlink index")?;
                Ok(Value::Sym(*sid))
            }
            b'@' => {
                let idx = self.long()?;
                self.objects
                    .get(usize::try_from(idx).map_err(|_| "bad object link".to_string())?)
                    .cloned()
                    .ok_or_else(|| "bad object link index".to_string())
            }
            b'I' => {
                // Ivar-wrapped object: the payload first, then the
                // ivar list. The encoding shorthand `:E` (true = UTF-8,
                // false = US-ASCII) is HONOURED — re-tagging the inner
                // string so a US-ASCII dump round-trips as US-ASCII
                // rather than collapsing to UTF-8. `:encoding "name"`
                // (registry encodings) is accepted but not applied
                // here; any other ivar name is out of subset.
                use crate::value::EncodingTag;
                let inner = self.read_value(vm)?;
                let n = self.long()?;
                let mut enc: Option<EncodingTag> = None;
                for _ in 0..n.max(0) {
                    let key = self.read_value(vm)?;
                    let val = self.read_value(vm)?;
                    let kname = match key {
                        Value::Sym(s) => vm.interner.resolve(s).to_string(),
                        _ => return Err("ivar key must be a symbol".into()),
                    };
                    match kname.as_str() {
                        "E" => {
                            enc = Some(match val {
                                Value::Bool(true) => EncodingTag::Utf8,
                                _ => EncodingTag::UsAscii,
                            });
                        }
                        "encoding" => {
                            let _ = val;
                        }
                        other => {
                            return Err(format!(
                                "unsupported marshal ivar :{other} (rubyrs load-only subset)"
                            ));
                        }
                    }
                }
                if let (Some(tag), Value::Str(rs)) = (enc, &inner) {
                    rs.encoding.set(tag);
                }
                Ok(inner)
            }
            b'[' => {
                let n = self.long()?;
                vm.maybe_gc();
                vm.check_alloc().map_err(|_| "allocation limit".to_string())?;
                let id = vm.heap.alloc(crate::heap::HeapObj::Array(Vec::new().into()));
                // Register BEFORE children (CRuby's link order) and
                // pin via the registration itself — `objects` is
                // walked by nothing, so pin explicitly around child
                // allocs instead: push to vm.pinned for the scope.
                // Register + pin BEFORE children; released by the
                // caller's pin_base truncate once the whole graph
                // is wired (popping here would expose a completed
                // child to GC while it's only held by the parent's
                // Rust-local buffer — the unicode.data UAF).
                self.objects.push(Value::Array(id));
                vm.pinned.push(Value::Array(id));
                let mut elems: Vec<Value> = Vec::with_capacity(n.max(0) as usize);
                for _ in 0..n.max(0) {
                    elems.push(self.read_value(vm)?);
                }
                if let crate::heap::HeapObj::Array(a) = vm.heap.get_mut(id) {
                    a.elems = elems;
                }
                Ok(Value::Array(id))
            }
            b'{' | b'}' => {
                // `{` = plain Hash; `}` = Hash with a scalar default
                // (`Hash.new(0)`), whose default value trails the pairs.
                let with_default = tag == b'}';
                let n = self.long()?;
                vm.maybe_gc();
                vm.check_alloc().map_err(|_| "allocation limit".to_string())?;
                let id = vm
                    .heap
                    .alloc(crate::heap::HeapObj::Hash(crate::heap::HashObj::with_pairs(Vec::new())));
                self.objects.push(Value::Hash(id));
                vm.pinned.push(Value::Hash(id));
                // A `C :Hash` wrapper means compare_by_identity: flag the
                // hash BEFORE the pair inserts so `vm_hash_insert` keys by
                // identity (identity-duplicate keys survive, like CRuby).
                if std::mem::take(&mut self.next_hash_by_identity)
                    && let crate::heap::HeapObj::Hash(h) = vm.heap.get(id)
                {
                    h.by_identity.set(true);
                }
                for _ in 0..n.max(0) {
                    let k = self.read_value(vm)?;
                    let v = self.read_value(vm)?;
                    // Insert-with-Hash-semantics, like CRuby's load (which
                    // asets each pair): duplicate keys — plain OR
                    // user-`hash`/`eql?` (whose `hash` IS dispatched during
                    // load, matching CRuby) — collapse instead of leaking
                    // raw duplicate pairs into the rebuilt table. Also
                    // roots each pair immediately (the old scratch Vec was
                    // unrooted across subsequent `read_value` allocations).
                    if let Err(t) = vm.vm_hash_insert(id, k, v) {
                        // Stash the Trap so the load entry point re-raises
                        // the USER's exception (catchable, like CRuby) —
                        // the reader's `_load` / `marshal_load` pattern.
                        // A String error here would collapse it into an
                        // uncatchable generic TypeError.
                        self.trap = Some(t);
                        return Err("hash key raised in Marshal.load".into());
                    }
                }
                let default = if with_default {
                    Some(self.read_value(vm)?)
                } else {
                    None
                };
                if let crate::heap::HeapObj::Hash(h) = vm.heap.get_mut(id)
                    && (default.is_some() || h.extras().is_some())
                {
                    h.extras_mut().default_value = default;
                }
                Ok(Value::Hash(id))
            }
            b'C' => {
                // Subclass-of-builtin wrapper: class symbol then the
                // underlying builtin value (`[`/`{`/`}`). Reconstruct the
                // builtin, then stamp the subclass onto its `class_tag` so
                // `inst.class` reports the subclass. (String subclasses —
                // CRuby's `I C :Sub "…"` — aren't modelled: rubyrs strings
                // carry no subclass tag; an `inner` that isn't an Array or
                // Hash is rejected.)
                let class_val = self.read_value(vm)?;
                let cname = match class_val {
                    Value::Sym(s) => vm.interner.resolve(s).to_string(),
                    _ => return Err("subclass tag must be a symbol".into()),
                };
                let cls = Self::marshal_resolve_cname(vm, &cname)?;
                // `C :Hash` (class IS the builtin) = compare_by_identity —
                // arm the flag so the inner `{`/`}` arm applies it before
                // inserting pairs. Cleared unconditionally after the read
                // (a malformed non-Hash payload must not leak it).
                if cname == "Hash" {
                    self.next_hash_by_identity = true;
                }
                let inner = self.read_value(vm)?;
                self.next_hash_by_identity = false;
                match &inner {
                    Value::Array(aid) => {
                        if let crate::heap::HeapObj::Array(a) = vm.heap.get_mut(*aid) {
                            a.class_tag = Some(cls);
                        }
                    }
                    Value::Hash(hid) => {
                        if let crate::heap::HeapObj::Hash(h) = vm.heap.get_mut(*hid) {
                            if cname == "Hash" {
                                // `C :Hash {…}` is CRuby's encoding of a
                                // compare_by_identity Hash (the class is the
                                // builtin itself, not a subclass) — restore
                                // the flag instead of a bogus class tag.
                                h.by_identity.set(true);
                            } else {
                                h.extras_mut().class_tag = Some(cls);
                            }
                        }
                    }
                    _ => return Err("unsupported `C` subclass payload (rubyrs: Array/Hash only)".into()),
                }
                Ok(inner)
            }
            b'u' => {
                // User-defined (`_dump`/`_load`): class symbol, then the
                // dump string (length-prefixed bytes). Reconstruct via the
                // class method `Class._load(str)`. A surrounding `I`
                // (encoding ivar on the dump string) is consumed by the
                // `I` arm; here we just read the raw bytes.
                let class_val = self.read_value(vm)?;
                let cls = self.marshal_lookup_class(vm, &class_val)?;
                let n = self.long()?;
                let data = self.take(n.max(0) as usize)?.to_vec();
                let data_str = match String::from_utf8(data) {
                    Ok(s) => Value::new_str(s),
                    Err(e) => Value::new_str_bytes_binary(e.into_bytes()),
                };
                let load_id = vm.interner.intern("_load");
                let pin_base = vm.pinned.len();
                vm.pinned.push(data_str.clone());
                let r = marshal_invoke(vm, Value::Class(cls), load_id, Some(data_str));
                vm.pinned.truncate(pin_base);
                let obj = match r {
                    Ok(v) => v,
                    Err(t) => {
                        self.trap = Some(t);
                        return Err("marshal _load raised".into());
                    }
                };
                self.objects.push(obj.clone());
                Ok(obj)
            }
            b'U' => {
                // User-marshal (`marshal_dump`/`marshal_load`): class
                // symbol, then the marshalled payload. Allocate an
                // instance, register it (so the payload can link back),
                // read the payload, then call `inst.marshal_load(payload)`.
                let class_val = self.read_value(vm)?;
                let cls = self.marshal_lookup_class(vm, &class_val)?;
                vm.maybe_gc();
                vm.check_alloc().map_err(|_| "allocation limit".to_string())?;
                let id = vm.heap.alloc(crate::heap::HeapObj::Instance(crate::value::Instance {
                    class: cls,
                    ivars: crate::value::IvarTable::default(),
                    singleton_class: None,
                    frozen: std::cell::Cell::new(false),
                }));
                self.objects.push(Value::Object(id));
                vm.pinned.push(Value::Object(id));
                let payload = self.read_value(vm)?;
                let load_id = vm.interner.intern("marshal_load");
                let pin_base = vm.pinned.len();
                vm.pinned.push(payload.clone());
                let r = marshal_invoke(vm, Value::Object(id), load_id, Some(payload));
                vm.pinned.truncate(pin_base);
                if let Err(t) = r {
                    self.trap = Some(t);
                    return Err("marshal marshal_load raised".into());
                }
                Ok(Value::Object(id))
            }
            b'o' => {
                // Generic object: class symbol, then ivar count and
                // [ivar_sym, value] pairs. Allocate an instance of the
                // named class (no `initialize` call, like CRuby) and set
                // the ivars. For an Exception descendant the bare `:mesg`
                // / `:bt` slots map back to rubyrs's `@message` /
                // `@backtrace`; every other key is a real `@`-prefixed
                // ivar name.
                let class_val = self.read_value(vm)?;
                let cname = match class_val {
                    Value::Sym(s) => vm.interner.resolve(s).to_string(),
                    _ => return Err("object class must be a symbol".into()),
                };
                // `o :Range` is CRuby's builtin-Range form (bare
                // `excl`/`begin`/`end` slots) — rubyrs Ranges are a
                // Value variant, not an Instance, so materialise the
                // heap RangeObj directly. Register a placeholder
                // BEFORE the endpoint reads (CRuby's link order), then
                // patch it in place.
                if cname == "Range" {
                    vm.maybe_gc();
                    vm.check_alloc().map_err(|_| "allocation limit".to_string())?;
                    let rid = vm.heap.alloc(crate::heap::HeapObj::Range(crate::heap::RangeObj {
                        begin: Value::Nil,
                        end: Value::Nil,
                        exclusive: false,
                    }));
                    self.objects.push(Value::Range(rid));
                    vm.pinned.push(Value::Range(rid));
                    let n = self.long()?;
                    for _ in 0..n.max(0) {
                        let key = self.read_value(vm)?;
                        let val = self.read_value(vm)?;
                        let kname = match key {
                            Value::Sym(s) => vm.interner.resolve(s).to_string(),
                            _ => return Err("ivar key must be a symbol".into()),
                        };
                        // Patch each slot IMMEDIATELY (not via Rust
                        // locals held across the next child read) —
                        // the pinned placeholder is what keeps an
                        // already-read endpoint alive through a GC
                        // triggered by the following read.
                        if let crate::heap::HeapObj::Range(r) = vm.heap.get_mut(rid) {
                            match kname.as_str() {
                                "excl" => r.exclusive = matches!(val, Value::Bool(true)),
                                "begin" => r.begin = val,
                                "end" => r.end = val,
                                other => {
                                    return Err(format!("unsupported Range slot :{other}"));
                                }
                            }
                        }
                    }
                    return Ok(Value::Range(rid));
                }
                let cls = Self::marshal_resolve_cname(vm, &cname)?;
                let is_exc = marshal_is_exception(&cls);
                vm.maybe_gc();
                vm.check_alloc().map_err(|_| "allocation limit".to_string())?;
                let id = vm.heap.alloc(crate::heap::HeapObj::Instance(crate::value::Instance {
                    class: cls,
                    ivars: crate::value::IvarTable::default(),
                    singleton_class: None,
                    frozen: std::cell::Cell::new(false),
                }));
                self.objects.push(Value::Object(id));
                vm.pinned.push(Value::Object(id));
                let n = self.long()?;
                for _ in 0..n.max(0) {
                    let key = self.read_value(vm)?;
                    let val = self.read_value(vm)?;
                    let kname = match key {
                        Value::Sym(s) => vm.interner.resolve(s).to_string(),
                        _ => return Err("ivar key must be a symbol".into()),
                    };
                    let (ivar, val) = if is_exc && kname == "mesg" {
                        // A nil :mesg means "default to the class name"
                        // (CRuby's lazy message). rubyrs reads @message
                        // raw, so materialise the class-name string now to
                        // keep `.message` correct for cross-loaded dumps.
                        let v = if matches!(val, Value::Nil) {
                            Value::new_str(cname.clone())
                        } else {
                            val
                        };
                        (vm.interner.intern("@message"), v)
                    } else if is_exc && kname == "bt" {
                        (vm.interner.intern("@backtrace"), val)
                    } else {
                        (vm.interner.intern(&kname), val)
                    };
                    if let crate::heap::HeapObj::Instance(inst) = vm.heap.get_mut(id) {
                        inst.ivar_set(ivar, val);
                    }
                }
                Ok(Value::Object(id))
            }
            b'S' => {
                // Struct: class symbol, then member count and
                // [member_sym, value] pairs. CRuby allocates the struct
                // and assigns members positionally (no `initialize`
                // call); we mirror that by setting the `@<member>` ivars
                // directly on a fresh instance of the named class. The
                // class must already be defined (CRuby raises otherwise).
                let class_val = self.read_value(vm)?;
                let cname = match class_val {
                    Value::Sym(s) => vm.interner.resolve(s).to_string(),
                    _ => return Err("struct class must be a symbol".into()),
                };
                let cid = vm.interner.intern(&cname);
                // A `Struct.new` class lives in the constants table under
                // its assigned name (it's anonymous to `vm.classes`), so
                // resolve the constant first, then fall back to the class
                // table for genuinely-named classes.
                let cls = match vm.constants.get(&cid) {
                    Some(Value::Class(c)) => c.clone(),
                    _ => vm
                        .classes
                        .get(&cid)
                        .cloned()
                        .ok_or_else(|| format!("undefined class/module {cname}"))?,
                };
                vm.maybe_gc();
                vm.check_alloc().map_err(|_| "allocation limit".to_string())?;
                let id = vm.heap.alloc(crate::heap::HeapObj::Instance(crate::value::Instance {
                    class: cls,
                    ivars: crate::value::IvarTable::default(),
                    singleton_class: None,
                    frozen: std::cell::Cell::new(false),
                }));
                self.objects.push(Value::Object(id));
                vm.pinned.push(Value::Object(id));
                let n = self.long()?;
                for _ in 0..n.max(0) {
                    let msym = self.read_value(vm)?;
                    let mval = self.read_value(vm)?;
                    let mname = match msym {
                        Value::Sym(s) => vm.interner.resolve(s).to_string(),
                        _ => return Err("struct member must be a symbol".into()),
                    };
                    let ivar = vm.interner.intern(&format!("@{mname}"));
                    if let crate::heap::HeapObj::Instance(inst) = vm.heap.get_mut(id) {
                        inst.ivar_set(ivar, mval);
                    }
                }
                Ok(Value::Object(id))
            }
            other => Err(format!(
                "unsupported marshal tag '{}' (rubyrs load-only subset: nil/bool/int/float/string/symbol/array/hash/struct)",
                other as char
            )),
        }
    }
}

/// Identity key for the marshal object-link table. The writer must
/// register an entry for EVERY object the reader registers (float,
/// string, array, hash) so `@`-link indices stay consistent with
/// CRuby's; `Opaque` is the placeholder for an object we can serialise
/// but never re-recognise (a Float has no Ruby-visible identity here).
#[derive(PartialEq)]
enum ObjKey {
    Heap(crate::value::ObjId),
    Str(usize),
    Opaque,
}

/// Binary `Marshal.dump` writer for the common-tag subset that
/// mirrors `MarshalReader` (nil/bool/Integer-in-i64/Float/String/
/// Symbol/Array/Hash). Output is byte-loadable by both this reader
/// and real CRuby, which makes `Marshal.load(Marshal.dump(x))` a
/// genuine DEEP COPY for these types (closing the documented
/// shallow-token divergence). Anything outside the subset — Bignum,
/// arbitrary objects, Struct, Range, Hash-with-default, a tagged
/// (non-UTF-8/ASCII/binary) string — makes `write_value` return
/// `Err(())`, and the caller falls back to the same-process registry
/// token (preserving the minitest identity contract for un-subset
/// types). Symbol + object link tables match CRuby's registration
/// order (parent before children), so shared substructure and cycles
/// serialise without infinite recursion.
struct MarshalWriter {
    out: Vec<u8>,
    symbols: Vec<crate::intern::SymId>,
    objects: Vec<ObjKey>,
    /// The interned `:E` symbol (marshal's encoding flag), interned by
    /// the caller so `write_value` can stay `&Vm` (read-only).
    e_sym: crate::intern::SymId,
    /// A trap raised by a reentrant `_dump` / `marshal_dump` hook.
    /// `write_value` returns `Err(())` and stashes the trap here so the
    /// caller can PROPAGATE the user exception (instead of silently
    /// falling back to the registry token, which would swallow it).
    trap: Option<Trap>,
}

/// Synchronously invoke a 0/1-arg Ruby method on `recv` from inside the
/// marshal writer/reader, running nested frames to completion. The
/// caller must keep `recv` and the in-flight graph GC-pinned. Returns
/// the result, or the raised `Trap` to propagate.
fn marshal_invoke(
    vm: &mut Vm,
    recv: Value,
    name: crate::intern::SymId,
    arg: Option<Value>,
) -> Result<Value, Trap> {
    let pre = vm.frames.len();
    vm.stack.push(recv);
    let argc = match arg {
        Some(a) => {
            vm.stack.push(a);
            1
        }
        None => 0,
    };
    vm.do_call(name, argc, false, u32::MAX)?;
    vm.dispatch_until(pre)?;
    Ok(vm.stack.pop().unwrap_or(Value::Nil))
}

impl MarshalWriter {
    /// Marshal's variable-length long (the inverse of
    /// `MarshalReader::long`). Small values fold into one byte;
    /// larger ones emit a signed count followed by little-endian
    /// payload bytes, stopping when the remaining value is 0
    /// (positive) or -1 (negative) — CRuby's `w_long`.
    fn write_long(&mut self, x: i64) {
        if x == 0 {
            self.out.push(0);
            return;
        }
        if (1..123).contains(&x) {
            self.out.push((x + 5) as u8);
            return;
        }
        if (-123..=-1).contains(&x) {
            self.out.push((x - 5) as i8 as u8);
            return;
        }
        let neg = x < 0;
        let mut v = x;
        let mut payload = [0u8; 8];
        let mut n = 0usize;
        loop {
            payload[n] = (v & 0xff) as u8;
            v >>= 8; // arithmetic shift (i64) preserves sign
            n += 1;
            if (!neg && v == 0) || (neg && v == -1) || n == 8 {
                break;
            }
        }
        let count: i64 = if neg { -(n as i64) } else { n as i64 };
        self.out.push(count as i8 as u8);
        self.out.extend_from_slice(&payload[..n]);
    }

    /// Symbol with the link table: first sighting writes `:`+text and
    /// registers; repeats write `;`+index.
    fn write_symbol(&mut self, vm: &Vm, sid: crate::intern::SymId) {
        if let Some(i) = self.symbols.iter().position(|&s| s == sid) {
            self.out.push(b';');
            self.write_long(i as i64);
            return;
        }
        self.symbols.push(sid);
        let name = vm.interner.resolve(sid).to_string();
        self.out.push(b':');
        self.write_long(name.len() as i64);
        self.out.extend_from_slice(name.as_bytes());
    }

    fn write_value(&mut self, vm: &mut Vm, v: &Value) -> Result<(), ()> {
        match v {
            Value::Nil => self.out.push(b'0'),
            Value::Bool(true) => self.out.push(b'T'),
            Value::Bool(false) => self.out.push(b'F'),
            Value::Int(n) => {
                // Marshal's `i` long form carries AT MOST 4 payload
                // bytes — CRuby dumps a 64-bit Fixnum outside i32 range
                // through the Bignum `l` tag instead (w_object's
                // RSHIFT(x,31) check). Mirroring that keeps the stream
                // well-formed (a 5-byte `i` payload desyncs any reader)
                // and CRuby-loadable.
                if i32::try_from(*n).is_ok() {
                    self.out.push(b'i');
                    self.write_long(*n);
                } else {
                    self.objects.push(ObjKey::Opaque);
                    self.out.push(b'l');
                    self.out.push(if *n < 0 { b'-' } else { b'+' });
                    let mut bytes = n.unsigned_abs().to_le_bytes().to_vec();
                    // Trim trailing zero 16-bit words (CRuby's shortlen).
                    while bytes.len() > 2 && bytes[bytes.len() - 1] == 0 && bytes[bytes.len() - 2] == 0 {
                        bytes.truncate(bytes.len() - 2);
                    }
                    self.write_long((bytes.len() / 2) as i64);
                    self.out.extend_from_slice(&bytes);
                }
            }
            Value::Float(f) => {
                self.objects.push(ObjKey::Opaque);
                self.out.push(b'f');
                let text = marshal_float_text(*f);
                self.write_long(text.len() as i64);
                self.out.extend_from_slice(text.as_bytes());
            }
            Value::Sym(sid) => self.write_symbol(vm, *sid),
            Value::Str(rs) => {
                let ptr = std::rc::Rc::as_ptr(rs) as *const () as usize;
                if let Some(i) = self.objects.iter().position(|k| *k == ObjKey::Str(ptr)) {
                    self.out.push(b'@');
                    self.write_long(i as i64);
                    return Ok(());
                }
                self.objects.push(ObjKey::Str(ptr));
                let bytes = rs.content.borrow().clone();
                use crate::value::EncodingTag;
                match rs.encoding.get() {
                    // Binary strings carry no encoding ivar (CRuby's
                    // ASCII-8BIT default) — a bare `"`.
                    EncodingTag::Binary => {
                        self.out.push(b'"');
                        self.write_long(bytes.len() as i64);
                        self.out.extend_from_slice(&bytes);
                    }
                    // UTF-8 / US-ASCII strings are ivar-wrapped with the
                    // `:E` flag (true = UTF-8, false = US-ASCII), exactly
                    // like CRuby. Registry-tagged strings (Other) fall
                    // out of subset.
                    EncodingTag::Utf8 | EncodingTag::UsAscii => {
                        let e_true = matches!(rs.encoding.get(), EncodingTag::Utf8);
                        self.out.push(b'I');
                        self.out.push(b'"');
                        self.write_long(bytes.len() as i64);
                        self.out.extend_from_slice(&bytes);
                        self.write_long(1); // one ivar
                        let e_sym = self.e_sym;
                        self.write_symbol(vm, e_sym);
                        self.out.push(if e_true { b'T' } else { b'F' });
                    }
                    EncodingTag::Other(_) => return Err(()),
                }
            }
            Value::Array(id) => {
                if let Some(i) = self.objects.iter().position(|k| *k == ObjKey::Heap(*id)) {
                    self.out.push(b'@');
                    self.write_long(i as i64);
                    return Ok(());
                }
                self.objects.push(ObjKey::Heap(*id));
                // An Array SUBCLASS (`class MyArr < Array`) is wrapped in
                // CRuby's `C` (class-of-builtin) tag: `C :MyArr <[…]>`.
                // The subclass must be named; anonymous → token fallback.
                if let Some(sub) = vm.heap.array_class_tag(*id) {
                    let csym = match marshal_class_sym(vm, &sub) {
                        Some(s) => s,
                        None => return Err(()),
                    };
                    self.out.push(b'C');
                    self.write_symbol(vm, csym);
                }
                let elems = vm.heap.array(*id).clone();
                self.out.push(b'[');
                self.write_long(elems.len() as i64);
                for e in &elems {
                    self.write_value(vm, e)?;
                }
            }
            Value::Hash(id) => {
                if let Some(i) = self.objects.iter().position(|k| *k == ObjKey::Heap(*id)) {
                    self.out.push(b'@');
                    self.write_long(i as i64);
                    return Ok(());
                }
                let (has_block, default, sub, by_id) = match vm.heap.get(*id) {
                    crate::heap::HeapObj::Hash(h) => {
                        (h.default_block().is_some(), h.default_value().cloned(),
                         h.class_tag().cloned(), h.by_identity.get())
                    }
                    _ => (false, None, None, false),
                };
                // A block default (`Hash.new { ... }`) wraps a Proc — not
                // serialisable; fall back to the token.
                if has_block {
                    return Err(());
                }
                self.objects.push(ObjKey::Heap(*id));
                // Hash SUBCLASS → `C` wrapper, like Array.
                if let Some(s) = &sub {
                    let csym = match marshal_class_sym(vm, s) {
                        Some(s) => s,
                        None => return Err(()),
                    };
                    self.out.push(b'C');
                    self.write_symbol(vm, csym);
                }
                if by_id {
                    // compare_by_identity — CRuby dumps it as `C :Hash {…}`
                    // (probed 3.4: `Marshal.dump({}.compare_by_identity)`
                    // is "\x04\bC:\tHash{\x00"); the loader restores the
                    // flag from the class-being-plain-Hash shape. A cbi
                    // SUBCLASS nests both wrappers: `C :Sub C :Hash {…}`
                    // (probed).
                    let csym = vm.interner.intern("Hash");
                    self.out.push(b'C');
                    self.write_symbol(vm, csym);
                }
                let pairs = vm.heap.hash(*id).to_vec();
                // A scalar default (`Hash.new(0)`) uses the `}` tag, which
                // trails the pairs with the default value; otherwise `{`.
                self.out.push(if default.is_some() { b'}' } else { b'{' });
                self.write_long(pairs.len() as i64);
                for (k, val) in &pairs {
                    self.write_value(vm, k)?;
                    self.write_value(vm, val)?;
                }
                if let Some(d) = default {
                    self.write_value(vm, &d)?;
                }
            }
            Value::Object(id) => {
                let inst_class = vm.heap.instance(*id).class.clone();
                // The class must be NAMED — a real constant name or one
                // lazily stamped on first const-assignment (`S =
                // Struct.new(...)` / `Foo = Class.new`). Anonymous →
                // CRuby raises; we fall back to the token.
                let cname = match inst_class.effective_name() {
                    Some(n) if !n.is_empty() => n,
                    _ => return Err(()),
                };
                let csym = match vm.interner.get_id(&cname) {
                    Some(s) => s,
                    None => return Err(()),
                };
                if let Some(i) = self.objects.iter().position(|k| *k == ObjKey::Heap(*id)) {
                    self.out.push(b'@');
                    self.write_long(i as i64);
                    return Ok(());
                }
                // User-defined marshal hooks take precedence over the
                // type-specific forms (CRuby's `w_object` checks
                // marshal_dump → `U`, then _dump → `u`, before `o`/`S`).
                let mdump_id = vm.interner.intern("marshal_dump");
                if vm.lookup_method_uncached(&inst_class, mdump_id).is_some() {
                    // `U` (user-marshal): class symbol, then the
                    // marshal_dump result serialised inline (sharing the
                    // link tables). marshal_load(that) rebuilds on read.
                    self.objects.push(ObjKey::Heap(*id));
                    self.out.push(b'U');
                    self.write_symbol(vm, csym);
                    let payload = match marshal_invoke(vm, Value::Object(*id), mdump_id, None) {
                        Ok(v) => v,
                        Err(t) => {
                            self.trap = Some(t);
                            return Err(());
                        }
                    };
                    let pin_base = vm.pinned.len();
                    vm.pinned.push(payload.clone());
                    let r = self.write_value(vm, &payload);
                    vm.pinned.truncate(pin_base);
                    return r;
                }
                let dump_id = vm.interner.intern("_dump");
                if vm.lookup_method_uncached(&inst_class, dump_id).is_some() {
                    // `u` (user-defined): class symbol, then the `_dump`
                    // result STRING (length-prefixed bytes). Wrapped in
                    // `I` to carry the string's encoding flag (UTF-8/
                    // US-ASCII), exactly like a plain String; a binary
                    // dump string needs no wrapper. `_load(str)` rebuilds.
                    self.objects.push(ObjKey::Heap(*id));
                    let dumped = match marshal_invoke(vm, Value::Object(*id), dump_id, Some(Value::Int(-1))) {
                        Ok(v) => v,
                        Err(t) => {
                            self.trap = Some(t);
                            return Err(());
                        }
                    };
                    let Value::Str(rs) = &dumped else { return Err(()) };
                    let bytes = rs.content.borrow().clone();
                    use crate::value::EncodingTag;
                    match rs.encoding.get() {
                        EncodingTag::Binary => {
                            self.out.push(b'u');
                            self.write_symbol(vm, csym);
                            self.write_long(bytes.len() as i64);
                            self.out.extend_from_slice(&bytes);
                        }
                        EncodingTag::Utf8 | EncodingTag::UsAscii => {
                            let e_true = matches!(rs.encoding.get(), EncodingTag::Utf8);
                            self.out.push(b'I');
                            self.out.push(b'u');
                            self.write_symbol(vm, csym);
                            self.write_long(bytes.len() as i64);
                            self.out.extend_from_slice(&bytes);
                            self.write_long(1);
                            let e_sym = self.e_sym;
                            self.write_symbol(vm, e_sym);
                            self.out.push(if e_true { b'T' } else { b'F' });
                        }
                        EncodingTag::Other(_) => return Err(()),
                    }
                    return Ok(());
                }
                // A Struct instance → CRuby's `S` tag: class symbol,
                // member count, then [member_sym, value] in declaration
                // order.
                if let Some(members) = marshal_struct_members(vm, &inst_class) {
                    self.objects.push(ObjKey::Heap(*id));
                    self.out.push(b'S');
                    self.write_symbol(vm, csym);
                    self.write_long(members.len() as i64);
                    for m in &members {
                        self.write_symbol(vm, *m);
                        let mname = vm.interner.resolve(*m).to_string();
                        let v = vm
                            .interner
                            .get_id(&format!("@{mname}"))
                            .and_then(|isym| vm.heap.instance(*id).ivar_get(isym).cloned())
                            .unwrap_or(Value::Nil);
                        self.write_value(vm, &v)?;
                    }
                    return Ok(());
                }
                // Otherwise a generic object → CRuby's `o` tag: class
                // symbol, ivar count, then [ivar_sym, value]. Ivars are
                // emitted name-sorted for run-to-run determinism (rubyrs
                // ivar storage is unordered; CRuby uses definition order,
                // so multi-ivar dumps may differ in byte ORDER but
                // round-trip identically).
                self.objects.push(ObjKey::Heap(*id));
                self.out.push(b'o');
                self.write_symbol(vm, csym);
                let is_exc = marshal_is_exception(&inst_class);
                // Snapshot ivars (clone out so later interning can take a
                // &mut borrow of the interner).
                let mut ivars: Vec<(crate::intern::SymId, Value)> = vm
                    .heap
                    .instance(*id)
                    .ivar_pairs()
                    .into_iter()
                    .map(|(k, v)| (k, v.clone()))
                    .collect();
                if is_exc {
                    // CRuby stores an Exception's message/backtrace in the
                    // bare `:mesg` / `:bt` slots (no `@`), NOT as ivars.
                    // rubyrs keeps them in `@message` / `@backtrace`
                    // (`@cause` is internal too) — translate on the way
                    // out and drop them from the user-ivar list.
                    let msg_ivar = vm.interner.intern("@message");
                    let bt_ivar = vm.interner.intern("@backtrace");
                    let cause_ivar = vm.interner.intern("@cause");
                    let mut msg = ivars.iter().find(|(k, _)| *k == msg_ivar)
                        .map(|(_, v)| v.clone()).unwrap_or(Value::Nil);
                    // CRuby lazily defaults a no-arg exception's message to
                    // the class name and dumps `:mesg nil`; rubyrs stores
                    // it eagerly in @message. Emit nil when @message just
                    // mirrors the class name so the bytes match CRuby (the
                    // reader maps a nil :mesg back to the class name, so
                    // round-trips stay correct either way).
                    if let Value::Str(s) = &msg
                        && *s.content.borrow() == *cname.as_bytes() {
                            msg = Value::Nil;
                        }
                    let bt = ivars.iter().find(|(k, _)| *k == bt_ivar)
                        .map(|(_, v)| v.clone()).unwrap_or(Value::Nil);
                    ivars.retain(|(k, _)| *k != msg_ivar && *k != bt_ivar && *k != cause_ivar);
                    ivars.sort_by(|a, b| vm.interner.resolve(a.0).cmp(vm.interner.resolve(b.0)));
                    let mesg_sym = vm.interner.intern("mesg");
                    let bt_sym = vm.interner.intern("bt");
                    self.write_long((2 + ivars.len()) as i64);
                    self.write_symbol(vm, mesg_sym);
                    self.write_value(vm, &msg)?;
                    self.write_symbol(vm, bt_sym);
                    self.write_value(vm, &bt)?;
                    for (k, v) in ivars.clone() {
                        self.write_symbol(vm, k);
                        self.write_value(vm, &v)?;
                    }
                } else {
                    ivars.sort_by(|a, b| vm.interner.resolve(a.0).cmp(vm.interner.resolve(b.0)));
                    self.write_long(ivars.len() as i64);
                    for (k, v) in ivars.clone() {
                        self.write_symbol(vm, k);
                        self.write_value(vm, &v)?;
                    }
                }
            }
            // Range → CRuby's builtin-object form: `o :Range` with the
            // bare `excl` / `begin` / `end` slots (no `@`), in CRuby's
            // field order. Registered in the object link table (CRuby
            // registers Ranges; a graph sharing one Range twice links).
            Value::Range(id) => {
                if let Some(i) = self.objects.iter().position(|k| *k == ObjKey::Heap(*id)) {
                    self.out.push(b'@');
                    self.write_long(i as i64);
                    return Ok(());
                }
                self.objects.push(ObjKey::Heap(*id));
                let (b, e, excl) = {
                    let r = vm.heap.range(*id);
                    (r.begin.clone(), r.end.clone(), r.exclusive)
                };
                self.out.push(b'o');
                let range_sym = vm.interner.intern("Range");
                self.write_symbol(vm, range_sym);
                self.write_long(3);
                let excl_sym = vm.interner.intern("excl");
                self.write_symbol(vm, excl_sym);
                self.out.push(if excl { b'T' } else { b'F' });
                let begin_sym = vm.interner.intern("begin");
                self.write_symbol(vm, begin_sym);
                self.write_value(vm, &b)?;
                let end_sym = vm.interner.intern("end");
                self.write_symbol(vm, end_sym);
                self.write_value(vm, &e)?;
            }
            // Bignum → CRuby's `l` tag: sign byte, length in 16-bit
            // words, magnitude little-endian. rubyrs's BigInt invariant
            // (always outside i64) means the writer never sees an
            // in-range one; the reader demotes to Int when it fits.
            #[cfg(feature = "bignum")]
            Value::BigInt(id) => {
                self.objects.push(ObjKey::Opaque);
                let big = vm.heap.bigint(*id).clone();
                self.out.push(b'l');
                let (sign, mag) = big.into_parts();
                self.out.push(if sign == num_bigint::Sign::Minus { b'-' } else { b'+' });
                let mut bytes = mag.to_bytes_le();
                if bytes.len() % 2 == 1 {
                    bytes.push(0);
                }
                self.write_long((bytes.len() / 2) as i64);
                self.out.extend_from_slice(&bytes);
            }
            // Block/Proc, Class, … — outside the byte subset; signal
            // the caller to use the registry token.
            _ => return Err(()),
        }
        Ok(())
    }
}

/// If `class` (or an ancestor) is a `Struct.new`-created class, return
/// its member symbols in declaration order (read from the
/// `@__struct_attrs` class ivar the Struct shim stores). `None` for a
/// plain object whose class isn't a Struct. Mirrors the chain walk the
/// preamble `Struct#members` does, so a Struct SUBCLASS instance still
/// finds the members on whichever ancestor `Struct.new` built.
fn marshal_struct_members(vm: &Vm, class: &std::rc::Rc<crate::value::Class>) -> Option<Vec<crate::intern::SymId>> {
    let attrs_id = vm.interner.get_id("@__struct_attrs")?;
    let mut cur = Some(class.clone());
    while let Some(c) = cur {
        if let Some(Value::Array(aid)) = c.ivars.borrow().get(&attrs_id).cloned() {
            let mut out = Vec::new();
            for e in vm.heap.array(aid) {
                match e {
                    Value::Sym(s) => out.push(*s),
                    _ => return None,
                }
            }
            return Some(out);
        }
        cur = c.superclass.borrow().clone();
    }
    None
}

/// Is `class` an `Exception` descendant? Mirrors the chain walk
/// `instance_variables` uses to hide `@message` / `@backtrace`. Drives
/// marshal's exception special-case (`:mesg` / `:bt` slots).
fn marshal_is_exception(class: &std::rc::Rc<crate::value::Class>) -> bool {
    let mut cur = Some(class.clone());
    while let Some(c) = cur {
        if c.name == "Exception" {
            return true;
        }
        cur = c.superclass.borrow().clone();
    }
    false
}

/// The interned symbol for a class's effective name, or `None` for an
/// anonymous class (no constant name) — the marshal writer's signal to
/// fall back to the registry token.
fn marshal_class_sym(vm: &Vm, cls: &std::rc::Rc<crate::value::Class>) -> Option<crate::intern::SymId> {
    let name = cls.effective_name()?;
    if name.is_empty() {
        return None;
    }
    vm.interner.get_id(&name)
}

/// CRuby marshal float text: `inf` / `-inf` / `nan` for the
/// non-finite cases, otherwise a shortest round-trippable decimal.
/// CRuby emits its `ruby_dtoa` shortest form (`100.0` → `"1e2"`); we
/// emit Rust's shortest `{}` form (`"100"`), which differs in spelling
/// but parses back to the identical f64 — and CRuby's own reader uses
/// `strtod`, so the bytes stay cross-loadable. `-0.0` → `"-0"`.
fn marshal_float_text(f: f64) -> String {
    if f.is_nan() {
        "nan".to_string()
    } else if f.is_infinite() {
        if f < 0.0 { "-inf".to_string() } else { "inf".to_string() }
    } else {
        format!("{f}")
    }
}

fn raise_system_exit(vm: &mut Vm, status: i32, message: &str) -> Option<Result<Value, Trap>> {
    // Look up SystemExit class. If the preamble hasn't loaded
    // (Phase 0.5a not yet in this build), surface a clear error
    // rather than panicking.
    let cls_id = vm.interner.intern("SystemExit");
    let cls = match vm.classes.get(&cls_id).cloned() {
        Some(c) => c,
        None => return Some(Err(vm.trap(RubyError::RuntimeError {
            msg: "SystemExit class missing — preamble Phase 0.5a not loaded".into(),
        }))),
    };
    vm.maybe_gc();
    if let Err(e) = vm.check_alloc() {
        return Some(Err(e));
    }
    // Allocate the instance, set @status + @message directly
    // (bypassing the Ruby-level `initialize` — equivalent end
    // state, no need to round-trip through invoke_method).
    let id = vm.heap.alloc(HeapObj::Instance(crate::value::Instance {
        class: cls,
        ivars: crate::value::IvarTable::default(),
        singleton_class: None,
            frozen: std::cell::Cell::new(false),
    }));
    let status_sym = vm.interner.intern("@status");
    let message_sym = vm.interner.intern("@message");
    let msg_val = Value::Str(std::rc::Rc::new(crate::value::RStr::new(message.to_string())));
    vm.heap.instance_mut(id).ivar_set(status_sym, Value::Int(status as i64));
    vm.heap.instance_mut(id).ivar_set(message_sym, msg_val);
    // Route through the same unwind path Op::Raise uses.
    if let Err(trap) = vm.unwind_with_exception(Value::Object(id)) {
        return Some(Err(trap));
    }
    // Unwind found a rescue. Don't push a Nil over it.
    vm.suppress_call_result_push = true;
    Some(Ok(Value::Nil))
}

/// ADR 0025 Phase 4a: convert a `SignalHandlerState` back to
/// a Ruby Value for `Signal.trap`'s previous-handler return.
/// Default → "DEFAULT" String; Ignore → "IGNORE" String;
/// Block(id) → Value::Block(id).
fn signal_handler_state_to_value(
    vm: &mut Vm,
    state: crate::vm::SignalHandlerState,
) -> Value {
    use crate::value::RStr;
    match state {
        crate::vm::SignalHandlerState::Default => {
            Value::Str(std::rc::Rc::new(RStr::new("DEFAULT".to_string())))
        }
        crate::vm::SignalHandlerState::Ignore => {
            Value::Str(std::rc::Rc::new(RStr::new("IGNORE".to_string())))
        }
        crate::vm::SignalHandlerState::Block(id) => {
            // Return the block as-is. Future Phase 4b will
            // re-invoke this block; for the return value we
            // hand the user back a reference they can pass
            // to a subsequent Signal.trap to restore.
            let _ = vm; // suppress unused-var when no allocation needed
            Value::Block(id)
        }
    }
}

/// Pure-computation core for `__rubyrs_time_parse_iso` (the
/// `Time.parse` fast path). Mirrors preamble/time.rb's hand-rolled
/// grammar — `YYYY-MM-DD`, optional `[ T]HH:MM[:SS]`, optional
/// `Z` / `±HH:MM` / `±HHMM` zone — but STRICTER: every numeric
/// field must be all-digits (the Ruby path's `to_i` accepts junk
/// suffixes / empty → 0 quirks), the year must fit well inside
/// i64 range, and anything else returns `None` so the Ruby parser
/// stays the single source of truth for edge shapes. For accepted
/// inputs the arithmetic is the identical Hinnant days_from_civil
/// (year is always >= 0 here, where Rust trunc-div and Ruby
/// floor-div agree; the `y - 399` pre-shift keeps the m<=2 → y=-1
/// case exact).
fn time_parse_iso(input: &str) -> Option<i64> {
    fn all_digits(s: &str) -> bool {
        !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
    }
    fn parse_num(s: &str) -> Option<i64> {
        if !all_digits(s) {
            return None;
        }
        // Cap well below i64::MAX so the seconds arithmetic below
        // can't overflow (the Ruby path auto-promotes to bignum;
        // we decline instead).
        if s.len() > 9 {
            return None;
        }
        s.parse::<i64>().ok()
    }
    fn days_from_civil(mut y: i64, m: i64, d: i64) -> i64 {
        if m <= 2 {
            y -= 1;
        }
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400;
        let mp = if m > 2 { m - 3 } else { m + 9 };
        let doy = (153 * mp + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }
    let s = input.trim();
    let sep = s.find([' ', 'T']);
    let date_str = sep.map_or(s, |i| &s[..i]);
    let mut rest = sep.map_or("", |i| s[i + 1..].trim());
    let mut dparts = date_str.split('-');
    let (y, m, d) = (dparts.next()?, dparts.next()?, dparts.next()?);
    if dparts.next().is_some() {
        return None;
    }
    let year = parse_num(y)?;
    let month = parse_num(m)?;
    let day = parse_num(d)?;
    let mut off: i64 = 0;
    let (mut hour, mut minute, mut second) = (0i64, 0i64, 0i64);
    if !rest.is_empty() {
        if let Some(stripped) = rest.strip_suffix('Z') {
            rest = stripped.trim_end();
        } else if let Some(tzpos) = rest.rfind(['+', '-']) {
            let tz = &rest[tzpos..];
            rest = rest[..tzpos].trim_end();
            let sign: i64 = if tz.starts_with('-') { -1 } else { 1 };
            let body = &tz[1..];
            // `±HHMM` or `±HH:MM`, digits only — else decline.
            let (oh, om) = match (body.len(), body.as_bytes().get(2)) {
                (5, Some(b':')) if all_digits(&body[..2]) && all_digits(&body[3..]) => {
                    (parse_num(&body[..2])?, parse_num(&body[3..])?)
                }
                (4, _) if all_digits(body) => (parse_num(&body[..2])?, parse_num(&body[2..])?),
                _ => return None,
            };
            off = sign * (oh * 3600 + om * 60);
        }
        if !rest.is_empty() {
            let mut tparts = rest.split(':');
            hour = parse_num(tparts.next()?)?;
            if let Some(t1) = tparts.next() {
                minute = parse_num(t1)?;
            }
            if let Some(t2) = tparts.next() {
                second = parse_num(t2)?;
            }
            if tparts.next().is_some() {
                return None;
            }
        }
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - off)
}

#[cfg(test)]
mod time_parse_iso_tests {
    use super::time_parse_iso;

    #[test]
    fn accepts_the_iso_shapes() {
        // Ground truth from the preamble Ruby parser / CRuby.
        assert_eq!(time_parse_iso("1970-01-01"), Some(0));
        assert_eq!(time_parse_iso("1970-01-02"), Some(86_400));
        assert_eq!(time_parse_iso("2024-03-15 10:30:00"), Some(1_710_498_600));
        assert_eq!(time_parse_iso("2024-03-15T10:30:00Z"), Some(1_710_498_600));
        assert_eq!(
            time_parse_iso("2024-03-15 10:30:00 +0800"),
            Some(1_710_498_600 - 8 * 3600)
        );
        assert_eq!(
            time_parse_iso("2024-03-15 10:30:00 -05:00"),
            Some(1_710_498_600 + 5 * 3600)
        );
        assert_eq!(time_parse_iso("  2024-03-15 10:30  "), Some(1_710_498_600));
        // pre-epoch
        assert_eq!(time_parse_iso("1969-12-31 23:59:59"), Some(-1));
        // m <= 2 / leap handling
        assert_eq!(time_parse_iso("2000-02-29"), Some(951_782_400));
    }

    #[test]
    fn declines_anything_loose() {
        for s in [
            "",
            "2024",
            "2024-01",
            "2024-01-02junk",
            "2024-ab-cd",
            "2024-01-02 10:30:00:99",
            "2024-01-02 10:30 +08",
            "2024-01-02 10:30 junk",
            "99999999999999999999-01-01",
            "2024-01-02-03",
        ] {
            assert_eq!(time_parse_iso(s), None, "should decline {s:?}");
        }
    }
}
