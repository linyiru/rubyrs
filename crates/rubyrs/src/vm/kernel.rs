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
                | "__method__"
                | "__callee__"
                | "block_given?"
                | "__defined_ivar?"
                | "__defined_method?"
                | "__defined_const?"
                | "__defined_recv_method?"
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
                        .ivars
                        .get(&delegate_sym)
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
        self.do_call(m_id, args.len(), /*no_recv=*/false, u16::MAX)?;
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
    fn bare_builtin_user_override(&mut self, name: &str) -> bool {
        let id = self.interner.intern(name);
        let self_val = self.frames.last().map(|f| f.self_val.clone());
        match &self_val {
            Some(Value::Object(oid)) => {
                let cls = self.heap.instance(*oid).class.clone();
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
                                        other.type_name(),
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
                                        other.type_name(),
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
                        // mirrors the existing nil-arg path at
                        // `Vm::arity_error_arg1_int` (gc.rs:292).
                        // Code-review #342 round 2.
                        let msg = match a {
                            Value::Nil => {
                                "no implicit conversion from nil to integer".to_string()
                            }
                            other => {
                                let type_name = match other {
                                    Value::Bool(true) => "true",
                                    Value::Bool(false) => "false",
                                    Value::Str(_) => "String",
                                    Value::Sym(_) => "Symbol",
                                    Value::Float(_) => "Float",
                                    Value::Array(_) => "Array",
                                    Value::Hash(_) => "Hash",
                                    o => o.type_name(),
                                };
                                format!(
                                    "no implicit conversion of {} into Integer",
                                    type_name,
                                )
                            }
                        };
                        return Some(Err(self.trap(RubyError::TypeError { msg })));
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
                // Resolve the enclosing method LEXICALLY — the same
                // `find_lexical_owner_frame` walk `yield` uses — not by
                // the call-stack-nearest method frame. They diverge when
                // the block runs through a user iterator that itself has
                // a block: `def helper; yield; end; def m; helper {
                // block_given? }; end` must report m's block, but the
                // nearest call-stack method is `helper` (which always has
                // a block). Matching yield's resolution keeps the two
                // consistent (and unblocks Enumerable methods that branch
                // on `block_given?` inside their `each { ... }` driver).
                let has_block = self
                    .lexical_owner_of_top()
                    .map(|idx| self.frames[idx].block_arg.is_some())
                    .unwrap_or(false);
                Some(Ok(Value::Bool(has_block)))
            }
            // `defined?` plumbing: three runtime checks that
            // resolve against `self` (ivars), the class chain
            // (methods), and the constant table. AST translation
            // routes here for IVarRead / Call / ConstRead inner
            // expressions. The label-only-on-hit pattern matches
            // CRuby: hit returns a String, miss returns nil.
            "__defined_ivar?" => {
                if let Some(Value::Sym(sid)) = args.first() {
                    let self_val = self.frames.last()
                        .map(|f| f.self_val.clone())
                        .unwrap_or(Value::Nil);
                    let hit = match &self_val {
                        Value::Object(oid) => {
                            self.heap.instance(*oid).ivars.contains_key(sid)
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
                        "__rubyrs_stdout_write" | "__rubyrs_stderr_write" |
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
                    other => return Some(Err(self.trap(RubyError::TypeError {
                        msg: format!("no implicit conversion of {} into Symbol", other.type_name()),
                    }))),
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
                        msg: format!("no implicit conversion of {} into String", other.type_name()),
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
                    other => return Some(Err(self.trap(RubyError::TypeError {
                        msg: format!("no implicit conversion of {} into Symbol", other.type_name()),
                    }))),
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
            "Integer" => {
                // Accept 1 or 2 args. The 2-arg form `Integer(str,
                // radix)` is the strict counterpart to
                // `String#to_i(radix)` — any garbage tail raises
                // ArgumentError where `to_i` would silently
                // accept a prefix and stop. Radix 0 means
                // "auto-detect via 0x/0o/0b/0d prefix"; 2..=36 is
                // the explicit form; anything else raises
                // ArgumentError.
                if args.is_empty() || args.len() > 2 {
                    return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: format!(
                            "wrong number of arguments (given {}, expected 1..2)",
                            args.len(),
                        ),
                    })));
                }
                let radix_arg: Option<i64> = if args.len() == 2 {
                    match &args[1] {
                        Value::Int(r) => Some(*r),
                        other => {
                            return Some(Err(self.trap(RubyError::TypeError {
                                msg: format!(
                                    "no implicit conversion of {} into Integer",
                                    other.type_name(),
                                ),
                            })));
                        }
                    }
                } else { None };
                // Validate the radix early — `Integer(non-str, 16)`
                // is a TypeError on the receiver and never reaches
                // the parse path, but `Integer("ff", 1)` is an
                // ArgumentError on the radix.
                if let Some(r) = radix_arg
                    && r != 0
                    && !(2..=36).contains(&r)
                {
                    return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: format!("invalid radix {}", r),
                    })));
                }
                let result = match (&args[0], radix_arg) {
                    // 1-arg form: receiver-shape dispatch, same
                    // as before. The 2-arg form REQUIRES a String
                    // (or Symbol — but CRuby doesn't accept those
                    // here either, so we don't).
                    (Value::Int(n), None) => Ok(Value::Int(*n)),
                    (Value::Float(f), None) => {
                        // CRuby raises FloatDomainError (a RangeError
                        // subclass) for NaN / ±Infinity here, matching
                        // `Float#to_i`'s shape — same message label so
                        // `Integer(Float::NAN)` and `Float::NAN.to_i`
                        // emit the same exception class, not divergent
                        // ones.
                        if !f.is_finite() {
                            Err(RubyError::FloatDomainError {
                                msg: crate::vm::numeric::float_domain_label(*f).to_string(),
                            })
                        } else { Ok(Value::Int(*f as i64)) }
                    }
                    (Value::Str(s), None) => {
                        let raw = s.to_string_lossy();
                        let trimmed = raw.trim();
                        match trimmed.parse::<i64>() {
                            Ok(n) => Ok(Value::Int(n)),
                            Err(_) => Err(RubyError::ArgumentError {
                                msg: format!("invalid value for Integer(): \"{}\"", raw),
                            }),
                        }
                    }
                    (Value::Nil, None) => Err(RubyError::TypeError {
                        msg: "can't convert nil into Integer".into(),
                    }),
                    (other, None) => Err(RubyError::TypeError {
                        msg: format!("can't convert {} into Integer", other.type_name()),
                    }),
                    // 2-arg form: only String accepted as the value.
                    (Value::Str(s), Some(radix)) => {
                        let raw = s.to_string_lossy();
                        match strict_parse_integer(&raw, radix) {
                            Some(n) => Ok(Value::Int(n)),
                            None => Err(RubyError::ArgumentError {
                                msg: format!("invalid value for Integer(): \"{}\"", raw),
                            }),
                        }
                    }
                    (_other, Some(_)) => Err(RubyError::ArgumentError {
                        // CRuby's exact message for the
                        // `Integer(non_string, radix)` case is
                        // `"base specified for non string value"` —
                        // it's an ArgumentError, NOT a TypeError,
                        // because the radix only makes sense paired
                        // with a String to parse. Mirror the class
                        // so `rescue ArgumentError` catches both
                        // rubyrs and CRuby alike.
                        msg: "base specified for non string value".into(),
                    }),
                };
                Some(result.map_err(|e| self.trap(e)))
            }
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
                        msg: format!("can't convert {} into Float", other.type_name()),
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
                let s = args[0].to_display(&self.heap, &self.interner);
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
                                        msg: format!("can't convert {} into Rational", v.type_name()),
                                    })
                                }
                            }
                            _ => Err(RubyError::TypeError {
                                msg: format!("can't convert {} into Rational", v.type_name()),
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
                                        msg: format!("can't convert {} into Rational", v.type_name()),
                                    })
                                }
                            }
                            _ => Err(RubyError::TypeError {
                                msg: format!("can't convert {} into Rational", v.type_name()),
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
                    _ => {
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
            "sleep" => {
                // A user/stub override on self's class chain wins —
                // bare `sleep(10)` in CRuby is an ordinary Kernel
                // method, and minitest's `self.stub :sleep, nil`
                // installs one on the test instance's eigenclass.
                // Same cold-path gate shape as Op::Raise's; the
                // kernel-alias forwarder (the saved original) is
                // excluded so the restore cycle can't loop.
                {
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
                            other.type_name(),
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
                                let inner = self.heap.instance(id).ivars.get(&msg_sym).cloned()
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
            "warn" => {
                // Tier-1 2c: `Kernel#warn(*msgs)` writes each
                // argument + "\n" to `Vm::stderr`. CRuby joins
                // multiple args with newlines (one terminator
                // each, including trailing); `warn` accepts any
                // arity. Tier-1 simplification: ignores the
                // `uplevel:` / `category:` kwargs CRuby exposes
                // (not in the rubyrs subset yet) — positional
                // args only.
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
                if let Some(target) = self.stdio_redirect("$stderr", true) {
                    // Render to one buffer, forward as a single
                    // write — same shape as the redirected `p`.
                    let mut buf = String::new();
                    for arg in args {
                        let s = arg.to_display(&self.heap, &self.interner);
                        buf.push_str(&s);
                        if !s.ends_with('\n') {
                            buf.push('\n');
                        }
                    }
                    return Some(self.forward_stdio_call(target, "write", &[Value::new_str(buf)]));
                }
                for arg in args {
                    let s = arg.to_display(&self.heap, &self.interner);
                    if s.ends_with('\n') {
                        let _ = write!(self.stderr, "{s}");
                    } else {
                        let _ = writeln!(self.stderr, "{s}");
                    }
                }
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
                            self.heap.instance_mut(*id).ivars.insert(msg_id, args[1].clone());
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
                let mut st: i32 = 0;
                let r = unsafe { libc::waitpid(pid, &mut st, 0) };
                if r < 0 {
                    return Some(Err(self.trap(RubyError::HostException {
                        class_name: "Errno::ECHILD".to_string(),
                        message: format!("No child processes - waitpid({pid})"),
                    })));
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
                    Err(msg) => Some(Err(self.trap(RubyError::TypeError { msg }))),
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
                                other.type_name(),
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
            "require" => match args {
                [Value::Str(path)] => {
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
                                        ivars: std::cell::RefCell::new(crate::intern::FxHashMap::default()),
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
                        other.type_name()
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
                            canon = Some(resolved);
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
                        other.type_name()
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
                            other.type_name()
                        ),
                    })))
                }
                [Value::Str(src)] => {
                    let owned = src.to_string_lossy();
                    Some(self.eval_string(&owned, "(eval)", /*synthetic=*/true))
                }
                // Common 2-arg shape: `eval(src, binding)` — drop
                // binding silently per the documented divergence.
                [Value::Str(src), _binding] => {
                    let owned = src.to_string_lossy();
                    Some(self.eval_string(&owned, "(eval)", /*synthetic=*/true))
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
                            file_arg.type_name()
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
                        msg: format!(
                            "no implicit conversion of {} into Integer",
                            line_arg.type_name()
                        ),
                    })))
                }
                [Value::Str(src), _binding, Value::Str(file)]
                | [Value::Str(src), _binding, Value::Str(file), _] => {
                    let owned = src.to_string_lossy();
                    let fname = file.to_string_lossy();
                    // `synthetic=false`: caller supplied the
                    // filename explicitly. Pass through to keep
                    // `__FILE__` stable across repeated evals.
                    Some(self.eval_string(&owned, &fname, /*synthetic=*/false))
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
        let mut target = base_dir.join(path_str);
        if target.extension().is_none() {
            target.set_extension("rb");
        }
        // Canonicalise so the duplicate-load check works regardless
        // of `./` / `..` or relative-cwd shape.
        let canon = match std::fs::canonicalize(&target) {
            Ok(p) => p,
            Err(e) => return Err(self.trap(RubyError::RuntimeError {
                msg: format!("require_relative: cannot find {} ({})", target.display(), e),
            })),
        };
        // Allowlist scope: bool gate already fired at the dispatch
        // arm (check_load_allowed("require_relative", None) before
        // path string handling, F6 ordering). This second call
        // re-runs the bool gate (no-op when already passed) and
        // additionally rejects canon paths outside any configured
        // `Config::allowed_paths` prefix. Canon was already
        // symlink-resolved by `std::fs::canonicalize`, so we get
        // a true post-resolution prefix check.
        self.check_load_allowed("require_relative", Some(&canon))?;
        self.load_ruby_source_from_canon(canon)
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
                canon = Some(resolved);
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
        // `_kramdown_native` accelerator hook, same shape: Jekyll's
        // KramdownParser#load_dependencies requires
        // "kramdown-parser-gfm" AFTER Kramdown::JekyllDocument is
        // defined, so this is the earliest safe patch point. The shim
        // is `defined?(...)`-guarded and no-ops outside Jekyll.
        #[cfg(feature = "_kramdown_native")]
        if path_str == "kramdown-parser-gfm" && matches!(result, Ok(Value::Bool(true))) {
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
        let rb_form: PathBuf = if p.extension().is_none() {
            p.with_extension("rb")
        } else {
            p.to_path_buf()
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
        // Has-non-rb extension (.so/.dylib/…) — go straight to
        // cext, don't even consider .rb candidates. Matches the
        // pre-refactor behaviour.
        let p = std::path::Path::new(path_str);
        if let Some(ext) = p.extension().and_then(|e| e.to_str())
            && ext != "rb" {
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
            return Ok(Value::Bool(false));
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
        let entry = crate::compiler::compile_proto(
            "<require>".into(), vec![], &[prog], filename_rc,
            &mut self.protos, &mut self.interner, &mut self.cache_counter,
        );
        let cc = self.cache_counter as usize;
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
        self.frames.push(super::Frame {
            proto_idx: entry,
            ip: 0,
            locals: crate::vm::Locals::Shared(std::rc::Rc::new(std::cell::RefCell::new(
                super::vec_nil(self.protos[entry].n_locals as usize)
            ))),
            self_val: Value::Nil,
            base_sp: self.stack.len(),
            is_class_body: false,
            swap_return: None,
            block_arg: None,
            defining_class: None,
            lexical_cvar_class: None,
            #[cfg(feature = "regex")] saved_last_match: None,
            is_block: false,
            n_given_positional: 0,
            kw_given_mask: 0,
            aux: None,
            pending_yield: false,
            block_writeback: None,
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
        Ok(Value::Bool(true))
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
        // Fast-fail BEFORE any parse / AST / compile work when
        // the frame cap is already exhausted. CPU-bound parse of
        // a large untrusted eval string shouldn't run just to
        // fail at the frame push at the bottom.
        self.check_frames()?;
        let parse_result = ruby_prism::parse(source.as_bytes());
        let mut parse_errors = parse_result.errors().peekable();
        if parse_errors.peek().is_some() {
            let msg = crate::error::format_prism_errors(source, parse_errors);
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
        let entry = crate::compiler::compile_proto(
            "<eval>".into(), vec![], &[prog], filename_rc.clone(),
            &mut self.protos, &mut self.interner, &mut self.cache_counter,
        );
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
        let cc = self.cache_counter as usize;
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
        self.frames.push(super::Frame {
            proto_idx: entry,
            ip: 0,
            locals: crate::vm::Locals::Shared(std::rc::Rc::new(std::cell::RefCell::new(
                super::vec_nil(self.protos[entry].n_locals as usize)
            ))),
            self_val: match &class_ctx {
                Some(cls) => Value::Class(cls.clone()),
                None => Value::Nil,
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
            is_block: false,
            n_given_positional: 0,
            kw_given_mask: 0,
            aux: None,
            pending_yield: false,
            block_writeback: None,
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
                            if inst.class.name.is_empty()
                                || inst.class.name.starts_with("#<")
                            {
                                return Err("can't dump anonymous class".into());
                            }
                            for iv in inst.ivars.values() {
                                stack.push(iv.clone());
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
                            if h.default_block.is_some() {
                                return Err("can't dump hash with default proc".into());
                            }
                            for (k, val) in h.pairs.iter() {
                                stack.push(k.clone());
                                stack.push(val.clone());
                            }
                            for iv in h.ivars.values() {
                                stack.push(iv.clone());
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
        | "bigdecimal" | "monitor" | "erb"
        // `etc`: vendored Etc.nprocessors subset (stdlib_vendor/etc.rb)
        // — minitest requires it unconditionally at load.
        | "etc"
        // `timeout`: lenient shell (rack's spec_utils requires it
        // for one Timeout::Error assertion; Timeout.timeout itself
        // needs real preemption — out of the single-threaded
        // model). The vendored stub defines the constants so the
        // require + rescue-class references resolve.
        | "timeout"
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
        // by `register_sqlite_host_fns` at battery init time;
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

/// Strict base-aware integer parser used by `Kernel#Integer(str,
/// radix)`. CRuby semantics:
///
///   - Strip leading + trailing whitespace.
///   - Optional `+` / `-` sign.
///   - `radix == 0` consults the source's `0x` / `0o` / `0b` /
///     `0d` prefix to pick the radix (default `10` if no
///     prefix). `2..=36` is an explicit radix; if the source
///     carries a MATCHING prefix it's consumed, otherwise the
///     parse starts at the first non-sign char.
///   - `_` is allowed BETWEEN digits but not adjacent to the
///     sign / prefix / endpoints (CRuby's strict literal shape).
///   - Every remaining char must be a valid digit in the chosen
///     radix — otherwise the parse fails (unlike `String#to_i`'s
///     lenient "stop at first non-digit" rule).
///
/// Returns `Some(n)` on a successful parse, `None` otherwise.
/// Wrapping arithmetic on the accumulator matches the existing
/// `String#to_i` semantics; i64 overflow is silently wrapped at
/// the parse step (BigInt promotion at the Kernel level is a
/// follow-up).
fn strict_parse_integer(raw: &str, radix: i64) -> Option<i64> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let (sign, mut body) = match s.as_bytes().first() {
        Some(b'-') => (-1i64, &s[1..]),
        Some(b'+') => (1i64, &s[1..]),
        _ => (1i64, s),
    };
    if body.is_empty() {
        return None;
    }
    let mut effective_r: u32 = if radix == 0 { 10 } else { radix as u32 };
    let body_bytes = body.as_bytes();
    if body_bytes.len() >= 2 && body_bytes[0] == b'0' {
        let prefix_r: u32 = match body_bytes[1] {
            b'x' | b'X' => 16,
            b'b' | b'B' => 2,
            b'o' | b'O' => 8,
            b'd' | b'D' => 10,
            _ => 0,
        };
        if prefix_r != 0 && (radix == 0 || radix as u32 == prefix_r) {
            effective_r = prefix_r;
            body = &body[2..];
        }
    }
    // After all prefix handling, the body must have at least one
    // digit. Leading `_` is rejected (CRuby refuses `_` adjacent
    // to the sign / prefix boundary).
    if body.is_empty() || body.starts_with('_') || body.ends_with('_') {
        return None;
    }
    let mut n: i64 = 0;
    let mut prev_was_underscore = false;
    for c in body.chars() {
        if c == '_' {
            // Two underscores in a row, or right after the
            // boundary, isn't legal in a Ruby integer literal.
            if prev_was_underscore { return None; }
            prev_was_underscore = true;
            continue;
        }
        prev_was_underscore = false;
        match c.to_digit(effective_r) {
            Some(d) => {
                n = n.wrapping_mul(effective_r as i64)
                    .wrapping_add(d as i64);
            }
            None => return None, // any non-digit tail is a hard error
        }
    }
    Some(sign.wrapping_mul(n))
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
                msg: format!(
                    "no implicit conversion of {} into Integer",
                    other.type_name(),
                ),
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
                if let Some(Value::Int(n)) = inst.ivars.get(&status_sym) {
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

    fn read_value(&mut self, vm: &mut Vm) -> Result<Value, String> {
        let tag = self.byte()?;
        match tag {
            b'0' => Ok(Value::Nil),
            b'T' => Ok(Value::Bool(true)),
            b'F' => Ok(Value::Bool(false)),
            b'i' => Ok(Value::Int(self.long()?)),
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
                // ivar list. Only the encoding shorthands (:E true/
                // false, :encoding "name") are consumed — they're
                // presentation-only for our tagged strings; any
                // other ivar name is out of subset.
                let inner = self.read_value(vm)?;
                let n = self.long()?;
                for _ in 0..n.max(0) {
                    let key = self.read_value(vm)?;
                    let val = self.read_value(vm)?;
                    let kname = match key {
                        Value::Sym(s) => vm.interner.resolve(s).to_string(),
                        _ => return Err("ivar key must be a symbol".into()),
                    };
                    match kname.as_str() {
                        "E" | "encoding" => {
                            let _ = val;
                        }
                        other => {
                            return Err(format!(
                                "unsupported marshal ivar :{other} (rubyrs load-only subset)"
                            ));
                        }
                    }
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
            b'{' => {
                let n = self.long()?;
                vm.maybe_gc();
                vm.check_alloc().map_err(|_| "allocation limit".to_string())?;
                let id = vm
                    .heap
                    .alloc(crate::heap::HeapObj::Hash(crate::heap::HashObj::with_pairs(Vec::new())));
                self.objects.push(Value::Hash(id));
                vm.pinned.push(Value::Hash(id));
                let mut pairs: Vec<(Value, Value)> = Vec::with_capacity(n.max(0) as usize);
                for _ in 0..n.max(0) {
                    let k = self.read_value(vm)?;
                    let v = self.read_value(vm)?;
                    pairs.push((k, v));
                }
                if let crate::heap::HeapObj::Hash(h) = vm.heap.get_mut(id) {
                    h.pairs = pairs;
                }
                Ok(Value::Hash(id))
            }
            other => Err(format!(
                "unsupported marshal tag '{}' (rubyrs load-only subset: nil/bool/int/float/string/symbol/array/hash)",
                other as char
            )),
        }
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
        ivars: crate::intern::FxHashMap::default(),
        singleton_class: None,
            frozen: std::cell::Cell::new(false),
    }));
    let status_sym = vm.interner.intern("@status");
    let message_sym = vm.interner.intern("@message");
    let msg_val = Value::Str(std::rc::Rc::new(crate::value::RStr::new(message.to_string())));
    vm.heap.instance_mut(id).ivars.insert(status_sym, Value::Int(status as i64));
    vm.heap.instance_mut(id).ivars.insert(message_sym, msg_val);
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
