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
    pub(crate) fn builtin_call(&mut self, name: &str, args: &[Value]) -> Option<Result<Value, Trap>> {
        match name {
            "puts" => {
                // CRuby's `puts` flattens arrays: each element is
                // printed on its own line, recursively. Empty
                // string still gets a newline (so `puts ""` and
                // `puts` look identical). Empty array prints
                // nothing.
                fn puts_one(vm: &mut Vm, v: &Value) {
                    match v {
                        Value::Array(id) => {
                            let snapshot: Vec<Value> = vm.heap.array(*id).clone();
                            for item in &snapshot { puts_one(vm, item); }
                        }
                        _ => {
                            let s = v.to_display(&vm.heap, &vm.interner);
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
                }
                if args.is_empty() {
                    let _ = writeln!(self.stdout);
                } else {
                    for a in args {
                        let cloned = a.clone();
                        puts_one(self, &cloned);
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
            "__method__" | "__callee__" => {
                let name_opt: Option<String> = {
                    let mut found = None;
                    for f in self.frames.iter().rev() {
                        if f.is_block || f.is_class_body { continue; }
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
                let has_block = self.frames.iter().rev()
                    .find(|f| !f.is_block && !f.is_class_body)
                    .map(|f| f.block_arg.is_some())
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
                    let hit = if let Value::Object(oid) = self_val {
                        self.heap.instance(oid).ivars.contains_key(sid)
                    } else { false };
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
                        "puts" | "p" | "pp" | "print" | "require" |
                        "Integer" | "Float" | "String" | "Array" |
                        "__defined_ivar?" | "__defined_method?" | "__defined_const?"
                    );
                    let host_hit = self.host_fns.contains_key(sid);
                    let self_val = self.frames.last()
                        .map(|f| f.self_val.clone())
                        .unwrap_or(Value::Nil);
                    let class_hit = if let Value::Object(oid) = &self_val {
                        let cls = self.heap.instance(*oid).class.clone();
                        self.lookup_method_uncached(&cls, *sid).is_some()
                    } else { false };
                    let toplevel_hit = self.toplevel_methods.contains_key(sid);
                    let hit = is_builtin || host_hit || class_hit || toplevel_hit;
                    return Some(Ok(if hit { Value::new_str("method") } else { Value::Nil }));
                }
                Some(Ok(Value::Nil))
            }
            "__defined_const?" => {
                if let Some(Value::Sym(sid)) = args.first() {
                    let hit = self.classes.contains_key(sid);
                    return Some(Ok(if hit { Value::new_str("constant") } else { Value::Nil }));
                }
                Some(Ok(Value::Nil))
            }
            "p" | "pp" => {
                for a in args {
                    let s = a.to_inspect(&self.heap, &self.interner);
                    let _ = writeln!(self.stdout, "{}", s);
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
                        let id = g.vm.heap.alloc(HeapObj::Array(elems));
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
                if args.len() != 1 {
                    return Some(Err(self.trap(RubyError::ArgumentError {
                        msg: format!("wrong number of arguments (given {}, expected 1)", args.len()),
                    })));
                }
                let result = match &args[0] {
                    Value::Int(n) => Ok(Value::Int(*n)),
                    Value::Float(f) => {
                        if !f.is_finite() {
                            Err(RubyError::TypeError {
                                msg: format!("can't convert {} into Integer", crate::heap::format_float(*f)),
                            })
                        } else { Ok(Value::Int(*f as i64)) }
                    }
                    Value::Str(s) => {
                        let raw = s.to_string_lossy();
                        let trimmed = raw.trim();
                        match trimmed.parse::<i64>() {
                            Ok(n) => Ok(Value::Int(n)),
                            Err(_) => Err(RubyError::ArgumentError {
                                msg: format!("invalid value for Integer(): \"{}\"", raw),
                            }),
                        }
                    }
                    Value::Nil => Err(RubyError::TypeError {
                        msg: "can't convert nil into Integer".into(),
                    }),
                    other => Err(RubyError::TypeError {
                        msg: format!("can't convert {} into Integer", other.type_name()),
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
                        let id = self.heap.alloc(crate::heap::HeapObj::Array(Vec::new()));
                        Some(Ok(Value::Array(id)))
                    }
                    Value::Array(_) => Some(Ok(args[0].clone())),
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
                        let id = g.vm.heap.alloc(crate::heap::HeapObj::Array(vec![elt]));
                        Some(Ok(Value::Array(id)))
                    }
                }
            }
            "print" => {
                for a in args {
                    let s = a.to_display(&self.heap, &self.interner);
                    let _ = write!(self.stdout, "{}", s);
                }
                Some(Ok(Value::Nil))
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
                        // Probe for a `.rb` sibling first, regardless
                        // of cfg!("cext"). Walks the same candidate
                        // list `require_ruby` consults — cwd-relative,
                        // caller-source-dir, caller-source-parent
                        // (the cross-package "lib"-style hop). Lets
                        // `require 'rack/show_exceptions'` from
                        // `<root>/sinatra/show_exceptions.rb`
                        // resolve to `<root>/rack/show_exceptions.rb`
                        // without forcing the script to spell out
                        // `require_relative` paths.
                        let rb_found = self.find_ruby_source_candidate(&path_str);
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
                                        name: std::cell::RefCell::new(cname.to_string()),
                                        is_module,
                                        methods: std::cell::RefCell::new(std::collections::HashMap::new()),
                                        singleton_methods: std::cell::RefCell::new(std::collections::HashMap::new()),
                                        superclass: std::cell::RefCell::new(None),
                                        includes: std::cell::RefCell::new(Vec::new()),
                                        prepends: std::cell::RefCell::new(Vec::new()),
                                        class_vars: std::cell::RefCell::new(std::collections::HashMap::new()),
                                        ivars: std::cell::RefCell::new(std::collections::HashMap::new()),
                                        #[cfg(feature = "cext")]
                                        cext_alloc_func: std::cell::Cell::new(None),
                                    })
                                });
                            }
                            Some(Ok(Value::Bool(true)))
                        } else {
                            #[cfg(feature = "cext")]
                            { Some(self.cext_require(&path_str)) }
                            #[cfg(not(feature = "cext"))]
                            {
                                Some(Err(self.trap(RubyError::RuntimeError {
                                    msg: format!(
                                        "require: no .rb at {} and built without \
                                         `cext` feature for native extension fallback",
                                        path_str
                                    ),
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
                        let path = path.to_string_lossy();
                        Some(Err(self.trap(RubyError::RuntimeError {
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
                [Value::Str(path)] => Some(path.with_str_lossy(|s| self.require_relative(s))),
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
            "require_relative" => Some(Err(self.trap(RubyError::RuntimeError {
                msg: "require_relative: file I/O not available on wasm32-wasi".into(),
            }))),
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
        self.load_ruby_source_from_canon(canon)
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
        let canon_opt = candidates.iter().find_map(|c| std::fs::canonicalize(c).ok());
        let canon = match canon_opt {
            Some(c) => c,
            None => {
                let tried = candidates.iter()
                    .map(|c| c.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(self.trap(RubyError::RuntimeError {
                    msg: format!("require: cannot find {} (tried: {})", path_str, tried),
                }));
            }
        };
        self.load_ruby_source_from_canon(canon)
    }

    /// Search-path candidates for `require <path_str>`. First
    /// existing one wins. CRuby's canonical model walks
    /// `$LOAD_PATH` (gem install paths + stdlib + the running
    /// script's dir); rubyrs approximates the "co-located source
    /// tree" subset of that for the embeddable / single-tree
    /// DSL host case. Absolute paths shortcut the search.
    ///
    /// Order:
    ///   1. as-given (handles absolute paths + cwd-relative).
    ///   2. caller source file's directory + name.rb
    ///      (sibling: `require 'helpers'` from `lib/x.rb`
    ///      finds `lib/helpers.rb`).
    ///   3. caller source file's PARENT directory + name.rb
    ///      (cross-package "lib" hop: `require
    ///      'rack/show_exceptions'` from
    ///      `<root>/sinatra/show_exceptions.rb` finds
    ///      `<root>/rack/show_exceptions.rb`).
    ///   4. each `$LOAD_PATH` entry + name.rb (in order;
    ///      scripts opt into this by `$LOAD_PATH.unshift(dir)`
    ///      at boot). Approximates CRuby's `$LOAD_PATH` walk
    ///      for hand-managed source trees + gem-vendor
    ///      layouts.
    ///   5. raw input as last-resort defensive fallback when
    ///      auto-`.rb` extension was applied but didn't match.
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
            let caller_dir: Option<PathBuf> = self.frames.last().and_then(|f| {
                let fname = self.protos[f.proto_idx].filename.to_string();
                Path::new(&fname).parent().map(Path::to_path_buf)
            });
            if let Some(dir) = caller_dir {
                candidates.push(dir.join(&rb_form));
                if let Some(parent) = dir.parent() {
                    candidates.push(parent.join(&rb_form));
                }
            }
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
        self.ruby_source_candidates(path_str)
            .iter()
            .any(|c| c.exists())
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
        // Parse + AST translate. Errors surface as SyntaxError
        // through the standard Trap path.
        let parse_result = ruby_prism::parse(source.as_bytes());
        let parse_errors: Vec<_> = parse_result.errors().collect();
        if !parse_errors.is_empty() {
            let msg = parse_errors.iter()
                .map(|e| format!("{:?}", e)).collect::<Vec<_>>().join("; ");
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
        let filename_rc: std::rc::Rc<str> = std::rc::Rc::from(canon.to_string_lossy().into_owned());
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
            locals: std::rc::Rc::new(std::cell::RefCell::new(
                super::vec_nil(self.protos[entry].n_locals as usize)
            )),
            self_val: Value::Nil,
            base_sp: self.stack.len(),
            is_class_body: false,
            swap_return: None,
            block_arg: None,
            defining_class: None,
            is_block: false,
            n_given_positional: 0,
            rescues: vec![],
            loop_rescue_depths: vec![], loop_stack_depths: vec![],
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
            let val = self.method_return.take().unwrap();
            // Pop block frames (handling class_eval block + class
            // body bookkeeping).
            while let Some(f) = self.frames.last() {
                if !f.is_block { break; }
                if self.frames.len() <= depth_before + 1 {
                    // Next pop would be our <main> — escape.
                    break;
                }
                let f = self.frames.pop().unwrap();
                self.stack.truncate(f.base_sp);
                if f.is_class_body {
                    let _cls = self.class_stack.pop()
                        .expect("ICE: class_stack empty unwinding through class_eval (require/_relative)");
                    self.class_visibility_stack.pop();
                }
            }
            if self.frames.len() <= depth_before + 1 {
                // The next pop would be our <main>, meaning the
                // non-local return targets either our <main> itself
                // (treat as file-return-with-value) or something
                // above it (escapes). Either way, restore the
                // method_return signal and exit the loop — the
                // post-loop bookkeeping decides between
                // "successful load with this value as result" and
                // "outer unwind takes over".
                self.method_return = Some(val);
                break;
            }
            // Pop the enclosing method frame, mirroring dispatch.
            let f = self.frames.pop().unwrap();
            self.stack.truncate(f.base_sp);
            if f.is_class_body {
                let cls = self.class_stack.pop()
                    .expect("ICE: class_stack empty on method-return (require/_relative)");
                self.class_visibility_stack.pop();
                self.stack.push(Value::Class(cls));
            } else if let Some(r) = f.swap_return {
                self.stack.push(r);
            } else {
                self.stack.push(val);
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
        "tempfile" => &[("Tempfile", false)],
        "stringio" => &[("StringIO", false)],
        "fileutils" => &[("FileUtils", true)],
        "digest" => &[("Digest", true)],
        "digest/md5" | "digest/sha1" | "digest/sha2" => &[],
        "base64" => &[("Base64", true)],
        "securerandom" => &[("SecureRandom", true)],
        "json" => &[("JSON", true)],
        "yaml" => &[("YAML", true)],
        "date" => &[("Date", false), ("DateTime", false)],
        "time" => &[],
        "csv" => &[("CSV", false)],
        "optparse" => &[("OptionParser", false)],
        "english" | "English" => &[],
        "bigdecimal" => &[("BigDecimal", false)],
        "monitor" => &[("Monitor", false), ("MonitorMixin", true)],
        "erb" => &[("ERB", false)],
        "open3" => &[("Open3", true)],
        "shellwords" => &[("Shellwords", true)],
        "weakref" => &[("WeakRef", false)],
        "cgi" | "cgi/util" => &[("CGI", false)],
        _ => &[],
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
fn is_stdlib_stub_name(name: &str) -> bool {
    matches!(
        name,
        "uri" | "uri/generic" | "uri/common"
        | "set" | "logger" | "forwardable"
        | "singleton" | "delegate" | "ostruct"
        | "pathname" | "tempfile" | "stringio" | "fileutils"
        | "digest" | "digest/md5" | "digest/sha1" | "digest/sha2"
        | "base64" | "securerandom"
        | "json" | "yaml" | "date" | "time" | "csv"
        | "optparse" | "english" | "English"
        | "bigdecimal" | "monitor" | "erb"
        | "open3" | "shellwords" | "weakref"
        | "cgi" | "cgi/util"
    )
}
