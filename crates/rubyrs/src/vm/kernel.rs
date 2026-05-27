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
                | "sprintf"
                | "format"
                | "__time_now_raw"
                | "__method__"
                | "__callee__"
                | "block_given?"
                | "__defined_ivar?"
                | "__defined_method?"
                | "__defined_const?"
                | "eval"
        )
    }

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
                        "sprintf" | "format" | "__time_now_raw" |
                        "Integer" | "Float" | "String" | "Array" |
                        "eval" |
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
                        if !f.is_finite() {
                            Err(RubyError::TypeError {
                                msg: format!("can't convert {} into Integer", crate::heap::format_float(*f)),
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
                let id = self.heap.alloc(HeapObj::Array(arr));
                Some(Ok(Value::Array(id)))
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
                let fmt_args = &args[1..];
                let out = match crate::vm::ruby_sprintf(
                    &fmt, fmt_args, &self.heap, &self.interner, self.max_value_bytes,
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
                                        methods: std::cell::RefCell::new(std::collections::HashMap::new()),
                                        singleton_methods: std::cell::RefCell::new(std::collections::HashMap::new()),
                                        superclass: std::cell::RefCell::new(None),
                                        includes: std::cell::RefCell::new(Vec::new()),
                                        prepends: std::cell::RefCell::new(Vec::new()),
                                        singleton_prepends: std::cell::RefCell::new(Vec::new()),
                                        class_vars: std::cell::RefCell::new(std::collections::HashMap::new()),
                                        ivars: std::cell::RefCell::new(std::collections::HashMap::new()),
                                        #[cfg(feature = "cext")]
                                        cext_alloc_func: std::cell::Cell::new(None),
                                    })
                                });
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
                    Some(self.eval_string(&owned, "(eval)"))
                }
                // Common 2-arg shape: `eval(src, binding)` — drop
                // binding silently per the documented divergence.
                [Value::Str(src), _binding] => {
                    let owned = src.to_string_lossy();
                    Some(self.eval_string(&owned, "(eval)"))
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
                    Some(self.eval_string(&owned, &fname))
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
            let val = self.take_method_return().unwrap();
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
    pub(crate) fn eval_string(
        &mut self,
        source: &str,
        filename: &str,
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
        // our default labels (`(eval)`, `(class_eval)`): on
        // collision, append an incrementing `:N` suffix to the
        // source-table key so the prior entry is preserved for
        // backtraces / `Method#source_location`. User-supplied
        // filenames pass through unchanged so `__FILE__` keeps
        // returning the caller's name across repeated evals — a
        // `:N` suffix would leak into observable metadata
        // (`eval("__FILE__", nil, "foo.rb")` called twice should
        // both see "foo.rb", not "foo.rb:2"). The trade-off:
        // explicit-filename collisions clobber the source-table
        // entry (matching CRuby's actual behavior); the suffix
        // dedupe applies only to the common ephemeral case of
        // repeated bare `eval(...)` / `cls.class_eval(str)` calls.
        let is_default_label = filename == "(eval)" || filename == "(class_eval)";
        let mut effective_filename: String = filename.to_string();
        if is_default_label && self.sources.contains_key(effective_filename.as_str()) {
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
        // Same dispatch-loop shape as compile_and_run_source;
        // a non-local `return` defined INSIDE the eval'd string
        // should unwind locally, but a `return` escaping the
        // eval'd top level pops back into the caller's frame.
        loop {
            if let Err(trap) = self.dispatch_until(depth_before) {
                return Err(trap);
            }
            if self.method_return.is_none() {
                break;
            }
            let val = self.take_method_return().unwrap();
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
                }
            }
            if self.frames.len() <= depth_before + 1 {
                self.method_return = Some(val);
                break;
            }
            let f = self.frames.pop().unwrap();
            self.stack.truncate(f.base_sp);
            if f.is_class_body {
                let cls = self.class_stack.pop()
                    .expect("ICE: class_stack empty on method-return (eval_string)");
                self.class_visibility_stack.pop();
                self.stack.push(Value::Class(cls));
            } else if let Some(r) = f.swap_return {
                self.stack.push(r);
            } else {
                self.stack.push(val);
            }
        }
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
        "tempfile" => &[("Tempfile", false)],
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
        | "pathname" | "tempfile" | "stringio" | "strscan" | "fileutils"
        | "digest" | "digest/md5" | "digest/sha1" | "digest/sha2"
        | "base64" | "securerandom"
        | "json" | "yaml" | "date" | "time" | "csv"
        | "optparse" | "english" | "English"
        | "bigdecimal" | "monitor" | "erb"
        | "open3" | "shellwords" | "weakref"
        | "cgi" | "cgi/util"
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
