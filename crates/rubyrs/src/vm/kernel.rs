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
                | "__time_now_raw"
                | "sleep"
                | "exit"
                | "exit!"
                | "abort"
                | "warn"
                | "at_exit"
                | "__rubyrs_signal_trap"
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
                let id = self.heap.alloc(crate::heap::HeapObj::Array(out));
                Some(Ok(Value::Array(id)))
            }
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
                        "puts" | "p" | "pp" | "print" | "require" | "load" |
                        "sprintf" | "format" | "__time_now_raw" | "sleep" |
                        "exit" | "exit!" | "abort" | "warn" | "at_exit" | "__rubyrs_signal_trap" |
                        "Integer" | "Float" | "String" | "Array" | "Rational" |
                        "eval" | "caller" |
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
                if args.len() == 1 {
                    if let Value::Float(f) = &args[0] {
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
                        let id = self.heap.alloc(crate::heap::HeapObj::Array(Vec::new()));
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
                            let pair_id = g.vm.heap.alloc(crate::heap::HeapObj::Array(vec![k, v]));
                            let pair_val = Value::Array(pair_id);
                            g.pin(pair_val.clone());
                            entries.push(pair_val);
                        }
                        g.vm.maybe_gc();
                        if let Err(t) = g.vm.check_alloc() { return Some(Err(t)); }
                        let id = g.vm.heap.alloc(crate::heap::HeapObj::Array(entries));
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
                        let bang_sym = self.interner.intern("$!");
                        match self.globals.get(&bang_sym).cloned() {
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
            "warn" => {
                // Tier-1 2c: `Kernel#warn(*msgs)` writes each
                // argument + "\n" to `Vm::stderr`. CRuby joins
                // multiple args with newlines (one terminator
                // each, including trailing); `warn` accepts any
                // arity. Tier-1 simplification: ignores the
                // `uplevel:` / `category:` kwargs CRuby exposes
                // (not in the rubyrs subset yet) — positional
                // args only.
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
                        // and the cext fallback's "cannot find C ext" trap is
                        // RuntimeError — wrong class for `rescue LoadError`,
                        // and a more revealing message than the scope reject.
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
                        let rb_found = self.allow_filesystem_io
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
                                        methods: std::cell::RefCell::new(std::collections::HashMap::new()),
                                        singleton_methods: std::cell::RefCell::new(std::collections::HashMap::new()),
                                        superclass: std::cell::RefCell::new(None),
                                        includes: std::cell::RefCell::new(Vec::new()),
                                        prepends: std::cell::RefCell::new(Vec::new()),
                                        singleton_prepends: std::cell::RefCell::new(Vec::new()),
                                        singleton_view: std::cell::RefCell::new(None),
                                        singleton_target: std::cell::RefCell::new(None),
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
        self.load_ruby_source_from_canon(canon)
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
            loop_rescue_depths: vec![], loop_stack_depths: vec![], pending_yield: false, begin_rescue_depths: vec![],
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
                    Some(rc) => std::rc::Rc::ptr_eq(&f_ref.locals, rc),
                    None => true,  // legacy fallback
                };
                let f = self.frames.pop().unwrap();
                self.stack.truncate(f.base_sp);
                if f.is_class_body {
                    let cls = self.class_stack.pop()
                        .expect("ICE: class_stack empty unwinding through class_eval (require/_relative)");
                    self.class_visibility_stack.pop();
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
                if is_owner { break; }
            }
            if escaped {
                self.method_return = Some(val);
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
            loop_rescue_depths: vec![], loop_stack_depths: vec![], pending_yield: false, begin_rescue_depths: vec![],
            block_writeback: None,
        });
        // Same dispatch-loop shape as compile_and_run_source;
        // a non-local `return` defined INSIDE the eval'd string
        // should unwind locally, but a `return` escaping the
        // eval'd top level pops back into the caller's frame.
        loop {
            self.dispatch_until(depth_before)?;
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
        | "rack"
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
        ivars: std::collections::HashMap::new(),
        singleton_class: None,
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
