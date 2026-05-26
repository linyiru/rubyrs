//! Method dispatch and call setup. Mirrors the call-handling
//! machinery CRuby keeps in `vm_eval.c` / `vm_insnhelper.c` —
//! finding the target method on a receiver, pushing a frame,
//! threading args/block through, and routing to host fns or
//! to interpreter bodies.
//!
//! Contents:
//!   - `do_call` / `do_call_block` — the Op::Call entry points
//!     called from the opcode loop.
//!   - `invoke_method` / `invoke_method_with_block` — frame
//!     setup once the target Method has been resolved.
//!   - `invoke_block` — re-enter a captured block.
//!   - `cext_invoke_method` — bridge for C-ext re-entering the
//!     Ruby side via `rb_funcallv`.
//!   - `try_method_missing` — fallback dispatch path when the
//!     name lookup fails.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::error::{RubyError, Trap};
use crate::heap::HeapObj;
use crate::intern::SymId;
use crate::value::{Class, Instance, Method, ObjId, Value, Visibility};

#[cfg(all(feature = "cext", not(target_os = "wasi")))]
use super::with_vm_ptr_set;
use super::{
    primitive_call, value_cmp_v, vec_nil, visibility_from_name, Frame, HostFnSlot, PinGuard, Vm,
};
use crate::HostCtx;

impl Vm {
    /// Re-entrant dispatch entry for C extensions calling back into
    /// Ruby via `rb_funcall*`. Invokes `recv.method(args)` through
    /// the normal `do_call` path, leaving the result on the stack
    /// where the caller can pop it.
    ///
    /// Setup mirrors what the compiler emits for a Ruby-side
    /// `recv.method(args)`: push the receiver, then each argument,
    /// then call `do_call` with `no_recv = false`. After `do_call`
    /// the result sits on top of the operand stack — pop and return.
    ///
    /// `cache_id = u16::MAX` is a sentinel that
    /// `lookup_method_cached` treats as "no cache slot": the
    /// `idx < call_caches.len()` guard naturally fails (the table
    /// is bounded by the number of compiled `Op::Call` instructions
    /// — nowhere near 65535 in any realistic program), so both the
    /// read and writeback paths short-circuit. Without this sentinel
    /// a hard-coded `cache_id = 0` would poison whichever compiled
    /// call site got slot 0 — that site would silently dispatch
    /// whatever class/method the C ext last invoked. Future work:
    /// allocate a per-`(recv-class, method)` cache for cext calls
    /// if profiling shows the uncached path matters.
    #[cfg(all(feature = "cext", not(target_os = "wasi")))]
    pub(crate) fn cext_invoke_method(
        &mut self,
        recv: Value,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, Trap> {
        let name_id = self.interner.intern(method);
        let argc = args.len();
        self.stack.push(recv);
        for a in args {
            self.stack.push(a);
        }
        self.do_call(
            name_id,
            argc,
            /* no_recv = */ false,
            /* cache_id = */ u16::MAX,
        )?;
        Ok(self
            .stack
            .pop()
            .expect("ICE: cext_invoke_method: do_call produced no result"))
    }



    /// Look up `method_missing` on `recv`'s class chain. If found,
    /// prepend the missed `name_id` as a Symbol arg and invoke it
    /// (pushing a frame); returns `Ok(true)` so the caller can
    /// `return Ok(())` instead of raising. Returns `Ok(false)` when
    /// the receiver doesn't carry a `method_missing` (or isn't a
    /// `Value::Object`) — caller proceeds to raise NoMethodError.
    ///
    /// Scope of this PoC: only Object receivers (user instances).
    /// Primitive receivers (Int, Str, …) skip the lookup — adding
    /// per-primitive class chains is a follow-up.
    pub(crate) fn try_method_missing(
        &mut self,
        recv: &Value,
        name_id: SymId,
        args: Vec<Value>,
        block: Option<ObjId>,
    ) -> Result<bool, Trap> {
        let cls = match recv {
            Value::Object(id) => self.heap.class_of(*id),
            _ => return Ok(false),
        };
        let mm_id = self.interner.intern("method_missing");
        let m = match self.lookup_method_uncached(&cls, mm_id) {
            Some(m) => m,
            None => return Ok(false),
        };
        let mut new_args = Vec::with_capacity(args.len() + 1);
        new_args.push(Value::Sym(name_id));
        new_args.extend(args);
        self.invoke_method_with_block(m, recv.clone(), new_args, block)?;
        Ok(true)
    }



    /// Invoke a registered host fn (either v1 or v2 slot).
    ///
    /// V1 stashes `*mut Vm` via `with_vm_ptr_set` so a cext-style
    /// re-entrant `rb_funcall` can find the running VM (ADR 0013) —
    /// but only on builds where that re-entry channel actually
    /// exists, i.e. `all(feature = "cext", not(target_os = "wasi"))`.
    /// With `--no-default-features` (or on wasi) `with_vm_ptr_set`
    /// itself lives inside the cfg'd-off `mod cext`, so the V1 arm
    /// just calls `host(args)` directly; see the in-fn comment for
    /// the migration site if a non-cext V1 host ever needs TLS-Vm
    /// access. V1 closures hold no Rust borrow of `self` during the
    /// call, so the raw-ptr reborrow inside cext is the only access
    /// path and aliasing is well-defined.
    ///
    /// V2 deliberately does NOT call `with_vm_ptr_set`. The V2
    /// closure holds a `HostCtx` that borrows `&self.heap` for the
    /// duration of the call; if we *also* re-aimed CURRENT_VM_PTR at
    /// `self` and the closure reborrowed it as `&mut Vm`, that
    /// reborrow would alias the live `&self.heap` borrow — any heap
    /// mutation during the inner call could realloc the backing
    /// `Vec<HeapObj>` and dangle slices returned by
    /// `ctx.resolve_array` / `resolve_hash`.
    ///
    /// Note that `CURRENT_VM_PTR` may already be non-null on entry
    /// (an outer v1/cext frame set it), so the V2 arm is NOT
    /// asserting "TLS is null." The actual boundary is: the TLS is
    /// `pub(crate)`, so an external v2 closure has no language-level
    /// path to read it — the unsafe re-entry channel is unreachable
    /// to user code in the V2 slot. Skipping the overwrite here is
    /// the closing brick: even an internal future v2 helper would
    /// have to explicitly opt into touching the TLS, which is the
    /// point at which the soundness review is expected.
    ///
    /// cext bridges register as V1, so nothing legitimate needs the
    /// ptr from the V2 arm.
    fn invoke_host_fn(&mut self, slot: HostFnSlot, args: &[Value]) -> Result<Value, Trap> {
        match slot {
            HostFnSlot::V1(host) => {
                // V1 contract under the `cext` feature gate:
                // `with_vm_ptr_set` parks the Vm pointer in TLS so a
                // cext-bridge V1 host can re-enter the VM through
                // rb_funcallv. With cext off there is no rb_funcall
                // path to need it and `with_vm_ptr_set` itself lives
                // inside `mod cext`, so we just call the host body
                // directly. Today every legitimate V1 caller IS a
                // cext bridge (see the V1/V2 doc above), so the
                // contract change is invisible at runtime; if a
                // future non-cext V1 host needs TLS-Vm access, this
                // is the site to move `with_vm_ptr_set` out of
                // `mod cext` and lift the cfg gate.
                #[cfg(all(feature = "cext", not(target_os = "wasi")))]
                {
                    let vm_ptr: *mut Vm = self;
                    with_vm_ptr_set(vm_ptr, || host(args))
                }
                #[cfg(any(not(feature = "cext"), target_os = "wasi"))]
                { host(args) }
            }
            HostFnSlot::V2(host) => {
                let ctx = HostCtx::new(&self.heap, &self.interner);
                host(&ctx, args)
            }
        }
    }

    /// Parse the first arg of a `send` / `__send__` call as the
    /// target method name. Symbol passes through; String is
    /// interned (CRuby's transparent `to_sym` on the name arg).
    /// Anything else returns the CRuby-shape TypeError
    /// (`<inspect> is not a symbol nor a string`); zero args
    /// returns the CRuby-shape ArgumentError. Shared by all four
    /// send-recogniser sites (`do_call` / `do_call_block`, each
    /// with their no_recv and recv arms) so the validation +
    /// error formatting can't drift between paths.
    fn parse_send_target(&mut self, args: &[Value]) -> Result<SymId, Trap> {
        if args.is_empty() {
            return Err(self.trap(RubyError::ArgumentError {
                msg: "wrong number of arguments (given 0, expected 1+)".into(),
            }));
        }
        match &args[0] {
            Value::Sym(s) => Ok(*s),
            Value::Str(s) => {
                // Same `Config::max_symbols` cap as `String#to_sym`
                // (vm/string.rs:971) — without this, untrusted code
                // could grow the interner unbounded by calling
                // `send("dyn_#{i}")` in a loop. Existing symbols
                // always re-resolve; only fresh names count.
                let name = s.to_string_lossy();
                if let Some(max) = self.max_symbols
                    && !self.interner.contains(&name) && self.interner.len() >= max {
                        return Err(self.trap(RubyError::ResourceExhausted {
                            msg: format!("interner exhausted: {} symbols", max),
                        }));
                    }
                Ok(self.interner.intern(&name))
            }
            other => {
                let inspected = other.to_inspect(&self.heap, &self.interner);
                Err(self.trap(RubyError::TypeError {
                    msg: format!("{} is not a symbol nor a string", inspected),
                }))
            }
        }
    }

    pub(crate) fn do_call(&mut self, name_id: SymId, argc: usize, no_recv: bool, cache_id: u16) -> Result<(), Trap> {
        let name = self.interner.resolve(name_id).clone();
        // Consume `bypass_visibility_once` at the dispatch
        // boundary, before any arm runs. A naive consume-at-the-
        // vis-check would leak the flag whenever the dispatch
        // bottoms out without entering the Value::Object arm
        // (e.g. `send(:nonexistent)` on a primitive receiver
        // raises NoMethodError before the Object arm is reached
        // — the flag would survive and silently bypass the next
        // call's vis check).
        let bypass_visibility = std::mem::replace(&mut self.bypass_visibility_once, false);
        let split = self.stack.len() - argc;
        let args: Vec<Value> = self.stack.drain(split..).collect();
        let recv = if no_recv {
            None
        } else {
            Some(self.stack.pop().expect("ICE: stack underflow before do_call receiver"))
        };

        if no_recv {
            if let Some(res) = self.builtin_call(&name, &args) {
                let v = res?;
                // `require_relative` (and any future builtin that
                // could see its caller unwound to an outer `rescue`
                // mid-call) sets `suppress_call_result_push` to
                // signal "don't push my return value — the stack
                // is now the rescue handler's, not yours". Check +
                // clear here so the flag is one-shot.
                if self.suppress_call_result_push {
                    self.suppress_call_result_push = false;
                } else {
                    self.stack.push(v);
                }
                return Ok(());
            }
            if let Some(host) = self.host_fns.get(&name_id).cloned() {
                let v = self.invoke_host_fn(host, &args)?;
                self.stack.push(v);
                return Ok(());
            }
            // Bare `send(:foo)` / `__send__(:foo)` — CRuby treats
            // these as `self.send(:foo)`. Resolve target and re-aim
            // through `do_call` with `no_recv = true` so the call
            // routes through the same implicit-self lookup path the
            // bare-call arm uses below. User `def send` on the
            // surrounding self wins for `send` (reserved-name rule
            // applies only to `__send__`); when the lookup finds a
            // user override, skip the recogniser so the normal
            // implicit-self arm below invokes it.
            //
            // The visibility-bypass flag is irrelevant here — the
            // no_recv arm doesn't enforce private/protected (calls
            // with implicit-self are always allowed) — but we still
            // set it for parity with the receiver-form arm, so any
            // helper that later inspects the flag sees a consistent
            // shape.
            if matches!(&*name, "send" | "__send__") {
                let frame_self = self.frames.last()
                    .expect("ICE: do_call(no_recv) with empty frames")
                    .self_val.clone();
                let user_override = &*name == "send" && match &frame_self {
                    Value::Object(id) => {
                        let cls = self.heap.class_of(*id);
                        self.lookup_method_cached(&cls, name_id, cache_id).is_some()
                    }
                    Value::Class(c) => self.lookup_class_singleton_method(c, name_id).is_some(),
                    _ => false,
                };
                if !user_override {
                    let target_sym = self.parse_send_target(&args)?;
                    let new_argc = args.len() - 1;
                    self.bypass_visibility_once = true;
                    for a in args.into_iter().skip(1) {
                        self.stack.push(a);
                    }
                    return self.do_call(target_sym, new_argc, true, u16::MAX);
                }
            }
            // Bare `method(:foo)` — implicit-self capture. Same
            // shape as `obj.method(:foo)` (the receiver-form arm
            // below) but the receiver is the surrounding frame's
            // `self_val`. Lets `arr.map(&method(:foo))` work from
            // inside an instance method body without writing
            // `&self.method(:foo)`.
            let self_val = self.frames.last().expect("ICE: do_call with empty frames").self_val.clone();
            if &*name == "method" && args.len() == 1
                && let Value::Sym(bound_name_id) = &args[0] {
                    self.maybe_gc(); // allow: gc-rooting — BoundMethod holds `recv: self_val.clone()` (cloned from `frames.last().self_val`, which stays rooted via `self.frames` for the whole alloc window) and a primitive `SymId`; no unrooted slot at risk.
                    self.check_alloc()?;
                    let id = self.heap.alloc(HeapObj::BoundMethod {
                        recv: self_val.clone(),
                        name_id: *bound_name_id,
                    });
                    self.stack.push(Value::BoundMethod(id));
                    return Ok(());
                }
            if let Value::Object(id) = &self_val {
                let cls = self.heap.class_of(*id);
                if let Some(m) = self.lookup_method_cached(&cls, name_id, cache_id) {
                    self.invoke_method(m, self_val.clone(), args)?;
                    return Ok(());
                }
            }
            // `self` is a Class — inside a class singleton method
            // body (`def self.foo; bar; end` or `class << self; def
            // foo; bar; end; end`), a bare call to `bar` should
            // resolve against THIS class's own `singleton_methods`
            // table AND its superclass chain (so `Sub.foo` defined
            // via `def self.foo` is reachable from inside `Sub`'s
            // class methods even when foo lives on `Super`).
            // Without this arm the lookup fell through to
            // toplevel_methods only and produced
            // "undefined method ... for Class" — even though
            // `bar` was sitting right there on `self`.
            //
            // Uses the same `lookup_class_singleton_method` helper
            // as the explicit `cls.foo` dispatch (vm/dispatch.rs
            // ~660), so `self.bar` and bare `bar` resolve
            // identically.
            if let Value::Class(c) = &self_val
                && let Some(m) = self.lookup_class_singleton_method(c, name_id) {
                self.invoke_method(m, self_val.clone(), args)?;
                return Ok(());
            }
            // Bare calls on Class instances inside `class Foo
            // ... end` bodies and `def self.X` singleton methods.
            // Each whitelisted name has a receiver-form arm
            // further down `do_call` (Class.new allocator,
            // Class#name, Class#method_defined?, Class#
            // instance_method, ...). Without this bridge the
            // bare-call branch would fall through to
            // `toplevel_methods` and raise NoMethodError, even
            // though `self.foo` works fine. Vendored msgpack-
            // ruby surfaced two of these:
            //   - `def self.from_msgpack_ext(...); new(...); end`
            //     in timestamp.rb (bare `new`)
            //   - `class Symbol; if method_defined?(:name); ...`
            //     in symbol.rb (bare `method_defined?` inside an
            //     `if`/`else` at class-body top level)
            // Push self_val + the original args back onto the
            // stack and re-enter `do_call` with `no_recv=false`
            // so the receiver-form dispatch takes over. The
            // whitelist matches lookup.rs's `Value::Class(_)`
            // primitive-method set — keep both in lockstep.
            if matches!(&self_val, Value::Class(_))
                && matches!(&*name, "new" | "name" | "method_defined?" | "instance_method" | "undef_method") {
                let argc = args.len();
                self.stack.push(self_val.clone());
                for a in args { self.stack.push(a); }
                return self.do_call(name_id, argc, /*no_recv=*/false, cache_id);
            }
            if let Some(m) = self.toplevel_methods.get(&name_id).cloned() {
                self.invoke_method(m, self_val, args)?;
                return Ok(());
            }
            // `include Mod` inside a class body — `self` is the
            // class, name resolves with no receiver. Pushes the
            // source onto the target's `includes` chain (instead
            // of copying methods); `lookup_method_uncached` then
            // walks the chain at dispatch time. Bumps method_gen
            // so any monomorphic inline cache entry that thought
            // the class lacked the included methods invalidates.
            if (&*name == "include" || &*name == "extend") && !args.is_empty()
                && let Value::Class(target) = &self_val {
                    for a in &args {
                        let src = match a {
                            Value::Class(c) => c.clone(),
                            _ => return Err(self.trap(RubyError::TypeError {
                                msg: format!(
                                    "wrong argument type {} (expected Module)",
                                    a.type_name(),
                                ),
                            })),
                        };
                        // CRuby last-included-wins: push to the
                        // front so it's checked first by the lookup
                        // walk (which goes head-to-tail).
                        let mut chain = target.includes.borrow_mut();
                        if !chain.iter().any(|c| Rc::ptr_eq(c, &src)) {
                            chain.insert(0, src);
                        }
                    }
                    self.method_gen = self.method_gen.wrapping_add(1);
                    self.stack.push(self_val.clone());
                    return Ok(());
                }
            // `private` / `protected` / `public` inside a class
            // body. With no args, switch the current visibility
            // mode for any subsequent `def`s. With Symbol or
            // String args, retroactively flip the visibility of
            // the listed methods on the current class. Outside a
            // class body these are no-ops returning nil — same
            // shape as CRuby's Module#private at the toplevel.
            if let Some(vis) = visibility_from_name(&name) {
                if let Value::Class(cls) = &self_val {
                    if args.is_empty() {
                        if let Some(top) = self.class_visibility_stack.last_mut() {
                            *top = vis;
                        }
                    } else {
                        let methods = cls.methods.borrow();
                        for a in &args {
                            let key: Option<SymId> = match a {
                                Value::Sym(s) => Some(*s),
                                Value::Str(s) => Some(self.interner.intern(&s.to_string_lossy())),
                                _ => None,
                            };
                            if let Some(mid) = key
                                && let Some(m) = methods.get(&mid) {
                                    m.visibility.set(vis);
                                }
                        }
                    }
                    self.stack.push(Value::Nil);
                    return Ok(());
                }
                // Toplevel `private` / `protected` / `public` —
                // CRuby treats these as visibility modifiers on
                // Object's singleton class. We don't model
                // toplevel methods as Object instance methods, so
                // the call has no observable effect; accept it as
                // a no-op rather than NoMethodError to keep
                // common preamble patterns (`private; def helper;`
                // at the toplevel) parseable.
                self.stack.push(Value::Nil);
                return Ok(());
            }
            // method_missing fallback (PoC #2). For Object self, look
            // up the class chain — if found, hand it the missed name
            // as a Symbol arg. Primitives skip this and raise directly.
            if self.try_method_missing(&self_val, name_id, args, None)? {
                return Ok(());
            }
            return Err(self.trap(RubyError::NoMethodError {
                method: name.to_string(), recv_type: self_val.type_name(),
            }));
        }

        let recv = recv.expect("ICE: receiver missing");

        // `Object#send(:name, args...)` / `__send__(:name, args...)`
        // — dynamic dispatch. Resolve the first arg as the target
        // method name and re-enter `do_call` with `recv` pushed
        // back, the remaining args on the stack, and the resolved
        // SymId in name_id. The whole normal lookup path then
        // handles the rest (primitives, singleton methods, host
        // fns, method_missing, etc.) — `send` is just a name
        // re-aim, not a separate dispatch table.
        //
        // The method-name arg accepts both Symbol and String
        // (CRuby's transparent `to_sym`). Same precedent as
        // `Object#method` but broader because shared specs and
        // tilt-style libraries commonly pass `send("foo")`.
        // Block-form (`send(:name) { ... }`) lives in
        // `do_call_block`; this arm covers the block-less call.
        //
        // cache_id passed as `u16::MAX` because the re-entered call
        // resolves a runtime-dynamic name — caching it at the
        // original `send` call site's slot would poison whatever
        // method the bytecode actually compiled for that slot.
        //
        // **CRuby parity — user-defined `def send`**: only
        // `__send__` is reserved. A user `def send` on the
        // receiver's class wins over the built-in re-aim when the
        // call is named `send`. We check that first and fall
        // through to the regular `Value::Object` arm if found.
        //
        // **CRuby parity — visibility bypass**: `send` and
        // `__send__` may invoke private/protected methods. Set
        // `bypass_visibility_once` to suppress the visibility
        // check during the re-entered call. The flag is consumed
        // (single-shot) at the top of the next `do_call` /
        // `do_call_block` into a local — *not* at the visibility
        // check site — so a dispatch that bottoms out before the
        // Object arm (e.g. `send(:nonexistent)` raising
        // NoMethodError on a primitive) can't leak the bypass
        // into the next unrelated call.
        let user_send_override = &*name == "send" && match &recv {
            Value::Object(id) => {
                let cls = self.heap.class_of(*id);
                self.lookup_method_cached(&cls, name_id, cache_id).is_some()
            }
            // `def self.send` on a class — singleton-method lookup
            // walking the class's superclass chain. Falls through to
            // the existing `Value::Class` arm which invokes the
            // user's singleton `send`.
            Value::Class(c) => self.lookup_class_singleton_method(c, name_id).is_some(),
            _ => false,
        };
        if matches!(&*name, "send" | "__send__") && !user_send_override {
            let target_sym = self.parse_send_target(&args)?;
            let new_argc = args.len() - 1;
            self.bypass_visibility_once = true;
            self.stack.push(recv);
            for a in args.into_iter().skip(1) {
                self.stack.push(a);
            }
            return self.do_call(target_sym, new_argc, false, u16::MAX);
        }

        if let Some(v) = primitive_call(&recv, &name, &args, self.max_value_bytes)
            .map_err(|e| self.trap(e))? {
            self.stack.push(v);
            return Ok(());
        }
        if let Some(v) = self.sym_primitive(&recv, &name, &args) {
            self.stack.push(v);
            return Ok(());
        }
        // BigInt method dispatch — `primitive_call` and friends
        // are stateless and can't read the heap, so the BigInt
        // surface is hooked here where `&mut self` is available.
        // Phase A: `to_s` and `inspect` (which produce a String
        // from the BigInt's decimal). Arithmetic + comparison
        // already get handled at the BinOp arm via the cold-path
        // `try_bigint_binop`; this hook covers method-call shape
        // (`big.to_s`, `big.inspect`, `big.send(:to_s)`).
        #[cfg(feature = "bignum")]
        if let Some(v) = self.bigint_primitive(&recv, &name, &args) {
            self.stack.push(v);
            return Ok(());
        }

        let new_id = self.interner.intern("new");
        if name_id == new_id
            && let Value::Class(cls) = &recv {
                // L3-F: cext-registered allocator path. When the class
                // came from rb_define_class_under AND the cext called
                // rb_define_alloc_func on it, route the allocation
                // through that callback (typically wraps a malloc'd C
                // struct in TypedData) instead of producing a bare
                // Instance. Without this, every TypedData_Get_Struct in
                // the cext's instance methods fails because `self` is a
                // plain Instance, not a TypedData slot.
                // Outer PinGuard covers BOTH the allocator call and
                // the subsequent initialize. cext_dispatch can trigger
                // maybe_gc (TypedData wrap, result translation,
                // nested rb_funcall); args + obj live only as Rust
                // locals here and would be swept otherwise (PR #50
                // review #1 + #3 — same shape as the Integer#times
                // PinGuard fix in L3-D).
                let mut g = PinGuard::new(self);
                for a in &args { g.pin(a.clone()); }
                // Default Instance allocator — used by every branch of
                // the cext-selection cascade below that doesn't go
                // through `rb_define_alloc_func`. Extracted so the
                // three call sites (cext non-wasi else arm, cext wasi
                // fallback, no-cext arm) can't drift out of sync.
                let alloc_instance = |g: &mut PinGuard, cls: &Rc<Class>| -> Result<Value, Trap> {
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    let id = g.vm.heap.alloc(HeapObj::Instance(Instance {
                        class: cls.clone(),
                        ivars: HashMap::new(),
                        singleton_class: None,
                    }));
                    Ok(Value::Object(id))
                };
                // Allocator selection. With `cext`, the class may carry
                // an `rb_define_alloc_func`-registered allocator that
                // must run instead of the default Instance allocation.
                // Without `cext`, there is no path that could set such
                // a function, so we collapse to the default allocator
                // unconditionally. Splitting the whole expression by
                // cfg (instead of the previous `Option<()>` sentinel
                // trick) keeps both arms well-typed and removes a
                // brittle `unreachable!()` site that any future
                // refactor inside the cfg arm could turn into a real
                // panic.
                #[cfg(feature = "cext")]
                let obj = if let Some(alloc_func) = cls.cext_alloc_func.get() {
                    #[cfg(not(target_os = "wasi"))]
                    {
                        // arity=0 (self-only) is the alloc_func ABI:
                        // VALUE allocate(VALUE klass). CURRENT_VM_PTR
                        // must be set so the cext can rb_funcall back
                        // and rb_data_typed_object_wrap can locate
                        // the Vm to allocate on its heap.
                        let class_name = cls.name.clone();
                        let qualified = format!("{}::allocate", class_name);
                        let vm_ptr: *mut Vm = g.vm;
                        let raw = super::cext::with_vm_ptr_set(vm_ptr, || {
                            super::cext::cext_dispatch(
                                &qualified,
                                alloc_func,
                                0,
                                &[],
                                super::cext::CextSelfHandle::Class(&class_name),
                            )
                        })?;
                        // PR #50 review #2: validate that the cext
                        // honored the rb_define_alloc_func contract.
                        // CRuby's allocator must return an Object
                        // (typically TypedData_Wrap_Struct'd); if a
                        // buggy cext returns Nil / a Class / an Int
                        // and we silently proceed, `initialize` is
                        // called on something that's not an instance,
                        // and instance-method dispatch later fails
                        // in a way that's hard to trace back to the
                        // allocator. Trap immediately with TypeError.
                        match &raw {
                            Value::Object(_) => raw,
                            other => {
                                let msg = format!(
                                    "allocator function for {} must return an Object, got {}",
                                    class_name,
                                    other.type_name()
                                );
                                return Err(g.vm.trap(RubyError::TypeError { msg }));
                            }
                        }
                    }
                    #[cfg(target_os = "wasi")]
                    {
                        // wasi: cext path is stubbed; fall back to
                        // plain Instance allocation. The `alloc_func`
                        // from the if-let binding is unused on this
                        // target (no cext_dispatch to forward it to);
                        // marker reference keeps -D warnings happy.
                        let _ = alloc_func;
                        alloc_instance(&mut g, cls)?
                    }
                } else {
                    alloc_instance(&mut g, cls)?
                };
                #[cfg(not(feature = "cext"))]
                let obj = {
                    // No cext_alloc_func field exists in this build;
                    // the class always allocates a plain Instance.
                    alloc_instance(&mut g, cls)?
                };
                // Pin the freshly-allocated obj across initialize so
                // a maybe_gc inside the (cext-defined or Ruby-defined)
                // initialize doesn't sweep it.
                g.pin(obj.clone());
                let init_id = g.vm.interner.intern("initialize");
                let ruby_init = g.vm.lookup_method_uncached(cls, init_id);
                if let Some(m) = ruby_init {
                    // Ruby-defined initialize takes precedence.
                    // Drop the guard before invoke_method (which
                    // needs &mut self uncontested); the pinned
                    // entries survive only the alloc step — by this
                    // point obj/args are already on Rust locals that
                    // invoke_method propagates.
                    drop(g);
                    self.invoke_method(m, obj.clone(), args)?;
                    self.frames.last_mut().expect("ICE: frames empty after new").swap_return = Some(obj);
                } else {
                    // L3-F + L3-H: cext-defined initialize (registered
                    // via rb_define_method) lives in
                    // cext_instance_methods. Dispatch through the
                    // existing instance-method path if present — this
                    // picks up arity validation and rb_raise handling
                    // for free. Both fixed arity 0..=5 AND variadic
                    // arity -1 are now dispatchable (L3-H setjmp shim
                    // supports case -1); the filter below mirrors
                    // cext_dispatch's accepted-arities rule.
                    #[cfg(all(feature = "cext", not(target_os = "wasi")))]
                    {
                        // PR #60 review #10: don't silently skip
                        // initialize on arity mismatch — that
                        // diverges from Ruby semantics
                        // (`Klass.new` must raise ArgumentError if
                        // the args don't fit initialize). Only
                        // filter on whether the arity is
                        // dispatchable by the setjmp shim at all
                        // ({-1} ∪ 0..=5); cext_dispatch then
                        // validates argc against arity for fixed
                        // cases and raises ArgumentError on a
                        // mismatch.
                        let cext_init_reg = g.vm.cext_instance_methods
                            .get(&cls.name)
                            .and_then(|t| t.get(&init_id).cloned())
                            .filter(|reg| reg.arity == -1 || (0..=5).contains(&reg.arity));
                        if let Some(reg) = cext_init_reg {
                            let qualified = reg.qualified_name.clone();
                            let func = reg.func;
                            let arity = reg.arity;
                            let obj_clone = obj.clone();
                            let args_ref = args.clone();
                            let vm_ptr: *mut Vm = g.vm;
                            super::cext::with_vm_ptr_set(vm_ptr, || {
                                super::cext::cext_dispatch(
                                    &qualified, func, arity, &args_ref,
                                    super::cext::CextSelfHandle::Object(obj_clone),
                                )
                            })?;
                        }
                    }
                    drop(g);
                    self.stack.push(obj);
                }
                return Ok(());
            }

        // Primitive-receiver fallback to the user-Class method
        // table. CRuby's dispatch walks every value's class chain
        // uniformly; rubyrs's primitive arms above handle the
        // built-in methods, but `class Symbol; alias_method
        // :to_msgpack_ext, :name; end` installs a forwarder in
        // `self.classes[Symbol].methods` that's only reachable
        // through the user-Class table. Look up the primitive's
        // class name via `class_of` and try `lookup_method_cached`
        // on it. Skip Object (its own arm below handles that) and
        // Class (Class.new etc. handled by the earlier arm).
        if !matches!(&recv, Value::Object(_) | Value::Class(_))
            && let Value::Class(cls) = self.class_of(&recv)
            && let Some(m) = self.lookup_method_cached(&cls, name_id, cache_id) {
            self.invoke_method(m, recv.clone(), args)?;
            return Ok(());
        }
        if let Value::Object(id) = &recv {
            let cls = self.heap.class_of(*id);
            if let Some(m) = self.lookup_method_cached(&cls, name_id, cache_id) {
                // Private methods cannot be invoked with an
                // explicit receiver. CRuby additionally allows
                // `self.foo` for some private writers; we keep
                // the simpler "any explicit receiver = denied"
                // rule.
                //
                // Protected methods can be invoked with an
                // explicit receiver only when the caller's `self`
                // is an instance of the receiver's class (or a
                // descendant). The common DSL use case is
                // `def >(other); other.balance > balance; end`
                // where both `self` and `other` are the same
                // class. We walk the current frame to find the
                // caller's self class and check kind_of? against
                // the receiver class via the existing
                // `class_is_a` helper.
                let vis = m.visibility.get();
                // `send` / `__send__` bypass the visibility check
                // exactly once. The flag was consumed at the top
                // of `do_call` into a local so it applies even when
                // the bypassed method itself dispatches other
                // calls (those see a freshly-cleared flag).
                if vis == Visibility::Private && !bypass_visibility {
                    return Err(self.trap(RubyError::NoMethodError {
                        method: format!("private method '{name}' called"),
                        recv_type: recv.type_name(),
                    }));
                }
                if vis == Visibility::Protected && !bypass_visibility {
                    // Check against the method's *defining* class
                    // (where `def name` literally lives) rather
                    // than the receiver's class — that's CRuby's
                    // rule. `a > c` where a=Account and
                    // c=SavingsAccount(<Account): inside `def >`
                    // the caller is an Account instance and the
                    // protected `balance` was defined on Account,
                    // so `Account.is_a?(Account)` is true and the
                    // call is allowed even though the receiver
                    // is a subclass.
                    let caller_self = self.frames.last()
                        .map(|f| f.self_val.clone())
                        .unwrap_or(Value::Nil);
                    let caller_cls = match &caller_self {
                        Value::Object(id) => Some(self.heap.class_of(*id)),
                        _ => None,
                    };
                    let defining = m.defining_class.as_ref().and_then(|w| w.upgrade());
                    let allowed = match (&caller_cls, &defining) {
                        (Some(c), Some(d)) => super::class_is_a(c, d),
                        _ => false,
                    };
                    if !allowed {
                        return Err(self.trap(RubyError::NoMethodError {
                            method: format!("protected method '{name}' called"),
                            recv_type: recv.type_name(),
                        }));
                    }
                }
                self.invoke_method(m, recv.clone(), args)?;
                return Ok(());
            }
            // L3-C: cext-registered instance method
            // (`rb_define_method`). Looked up AFTER script-defined
            // methods so a Ruby-side override wins for
            // concrete-class methods.
            //
            // **Known limitation** (review #1 on PR #27): the
            // current shape walks the script-method ancestor chain
            // via lookup_method_cached, THEN checks cext methods
            // only on the receiver's own class. So a Ruby method
            // on a superclass shadows a cext method on the
            // subclass, and a cext method on a superclass is
            // invisible to subclass instances. A complete fix
            // would interleave cext lookup INSIDE the per-class
            // walk in lookup_method_cached — out of L3-C wedge
            // scope. Real-world impact is small: the common pattern
            // is `class Foo; end` + `rb_define_method(Foo, ...)`
            // on the same class, which works correctly.
            #[cfg(all(feature = "cext", not(target_os = "wasi")))]
            {
                if let Some(table) = self.cext_instance_methods.get(&cls.name)
                    && let Some(reg) = table.get(&name_id).cloned() {
                        // Pin recv + args across the cext call
                        // (review #4 on PR #27). cext_dispatch may
                        // run maybe_gc during arg translation /
                        // TypedData wrapping / result translation;
                        // recv was popped from vm.stack before we
                        // got here, so without pinning a STRESS_GC
                        // sweep can reclaim it mid-call →
                        // use-after-free in the cext body. Same
                        // shape as the L1.5 P0-A pattern.
                        //
                        // RAII guard holding only a `*mut Vec<Value>`
                        // (not `&mut Vm`) so it doesn't conflict with
                        // the `vm_ptr: *mut Vm` we hand to
                        // `with_vm_ptr_set` — PinGuard's `&mut Vm`
                        // would alias under Stacked Borrows when
                        // cext_dispatch's rb_funcall reentrance
                        // re-derefs the raw pointer (same gotcha L3-A
                        // review #15 / PR #6 hit). The narrower
                        // pointer is sound because it borrows only
                        // the field, not the whole Vm.
                        //
                        // Truncate runs on Drop, so a panic from
                        // `with_vm_ptr_set` / `cext_dispatch` (or
                        // the trailing `?`) doesn't leak pinned
                        // entries — fixes review #11 on PR #27,
                        // where the prior manual push/truncate
                        // skipped truncate on the unwind path.
                        struct PinTruncateGuard {
                            pinned: *mut Vec<Value>,
                            saved_depth: usize,
                        }
                        impl Drop for PinTruncateGuard {
                            fn drop(&mut self) {
                                // SAFETY: `pinned` was taken from
                                // `&mut self.pinned` in the
                                // enclosing scope; the guard is
                                // dropped before that borrow could
                                // be used elsewhere, and no other
                                // Rust code mutates `pinned` while
                                // the cext call is on the stack.
                                unsafe { (*self.pinned).truncate(self.saved_depth); }
                            }
                        }
                        let saved_pin_depth = self.pinned.len();
                        self.pinned.push(recv.clone());
                        for a in &args { self.pinned.push(a.clone()); }
                        let _pin_guard = PinTruncateGuard {
                            pinned: &raw mut self.pinned,
                            saved_depth: saved_pin_depth,
                        };
                        let vm_ptr: *mut Vm = self;
                        let recv_clone = recv.clone();
                        let v = with_vm_ptr_set(vm_ptr, || {
                            crate::vm::cext::cext_dispatch(
                                &reg.qualified_name,
                                reg.func,
                                reg.arity,
                                &args,
                                crate::vm::cext::CextSelfHandle::Object(recv_clone),
                            )
                        })?;
                        // Explicit drop here is documentation, not
                        // necessity — `_pin_guard` drops at scope
                        // end either way.
                        drop(_pin_guard);
                        self.stack.push(v);
                        return Ok(());
                    }
            }
        }
        // C-ext singleton dispatch: `BCrypt::Engine.__bc_crypt(args)`
        // arrives here with recv = Value::Class(c). Look up the
        // method in the per-class cext table populated by
        // `Vm::cext_require` (rb_define_singleton_method).
        // File class-method shims. CRuby exposes File.read / .write
        // / .exist? / .open / .basename as class methods; we don't
        // have a `def self.foo` syntax yet, so the dispatch is a
        // hand-rolled intercept on the File class. I/O paths
        // surface OS errors as a generic RuntimeError so scripts
        // can `rescue` them.
        if let Value::Class(cls) = &recv {
            // User-Ruby `def self.foo` singletons: walk the
            // per-class `singleton_methods` table chain via the
            // shared helper. CRuby's metaclass model has the
            // singleton class of `Dog < Animal` inherit from the
            // singleton class of `Animal`, so `Dog.kingdom` finds
            // `Animal`'s `def self.kingdom`. The same helper is
            // used by the bare-call path (no_recv when self is a
            // Class) so `self.bar` and bare `bar` stay in sync.
            let user_singleton = self.lookup_class_singleton_method(cls, name_id);
            if let Some(m) = user_singleton {
                let target_self = recv.clone();
                return self.invoke_method(m, target_self, args);
            }
            if &*cls.name == "File"
                && let Some(v) = self.file_class_dispatch(&name, &args)? {
                    self.stack.push(v);
                    return Ok(());
                }
            #[cfg(feature = "cext")]
            if let Some(table) = self.cext_class_methods.get(&cls.name)
                && let Some(host) = table.get(&name_id).cloned() {
                    // Stash Vm pointer for the singleton-method's
                    // C body — same rationale as the top-level
                    // host_fns dispatch above.
                    #[cfg(not(target_os = "wasi"))]
                    let v = {
                        let vm_ptr: *mut Vm = self;
                        with_vm_ptr_set(vm_ptr, || host(&args))?
                    };
                    #[cfg(target_os = "wasi")]
                    let v = host(&args)?;
                    self.stack.push(v);
                    return Ok(());
                }
        }
        // `include Mod` — without real Modules in the subset, we
        // approximate by copying the source class's method table
        // into the target class. Only fills methods the target
        // doesn't already define, so user overrides win (matching
        // CRuby's ancestor-chain semantics where own methods
        // shadow included ones). Defines `include` ad-hoc on
        // Class receivers; the call is a no-op for any other
        // receiver and falls through to NoMethodError.
        // `proc.call(args)` / `lambda.call(args)` — invoke the
        // block synchronously and push its result. Sub-frame
        // runs until it returns; same dispatch shape as iterator
        // drivers' invoke_block + dispatch_until pattern, but
        // accessible from script code (rather than only from
        // builtin iterators).
        if let Value::Block(bid) = &recv
            && matches!(&*name, "call" | "[]" | "()" | "yield") {
                // CRuby exposes block invocation under four names:
                // `.call(args)`, `.()` (already lowered to `call`
                // by parsers but kept here defensively), `[args]`
                // bracket form, and `.yield(args)` (mostly a
                // documentation alias). All four route the same
                // way: invoke the block, drive until its frame
                // returns, leave the result on the stack.
                let pre_frames = self.frames.len();
                self.invoke_block(*bid, args)?;
                self.dispatch_until(pre_frames)?;
                return Ok(());
            }
        // `Object#method(:name)` — capture (recv, name_id) into a
        // BoundMethod heap object. Returned Value can be `.call`'d
        // (handled in the next arm) or stored. Args must be a
        // single Symbol; CRuby also accepts String but we keep
        // the subset narrow for now.
        //
        // GC rooting: `recv` here came from the operand-stack pop
        // at the top of `do_call` and lives only in this Rust
        // local. The `maybe_gc` below would otherwise sweep its
        // heap slot (e.g. a fresh `Squared.new.method(:call)`
        // where the Squared instance has no other root), then the
        // alloc'd BoundMethod would store a stale ObjId. Repro:
        // `proc_curry_compose.rb` under STRESS_GC=1 — the
        // BoundMethod survives but its `recv` points at a Dead
        // slot, panicking later in `class_of`.
        if &*name == "method" && args.len() == 1
            && let Value::Sym(bound_name_id) = &args[0] {
                let mut g = crate::vm::PinGuard::new(self);
                g.pin(recv.clone());
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let id = g.vm.heap.alloc(HeapObj::BoundMethod {
                    recv: recv.clone(),
                    name_id: *bound_name_id,
                });
                g.vm.stack.push(Value::BoundMethod(id));
                return Ok(());
            }
        // `bm.call(args)` / `bm.()` / `bm[args]` — dispatch the
        // captured method on the captured receiver. We re-enter
        // `do_call` recursively with the bound recv pushed below
        // the args, the captured name interned, and the original
        // argc.
        // `bm.unbind` — strip the receiver, keep (class_of(recv),
        // name_id). The captured class is the receiver's class at
        // unbind time; CRuby technically captures the *owner* (the
        // class that defined the method), but for our subset
        // `class_of` is the closest approximation and roundtrips
        // through `bind` correctly for the common shapes.
        if let Value::BoundMethod(bid) = &recv && &*name == "unbind" && args.is_empty() {
            let (bm_recv, bm_name_id) = match self.heap.get(*bid) {
                HeapObj::BoundMethod { recv, name_id } => (recv.clone(), *name_id),
                _ => panic!("ICE: BoundMethod slot holds non-BoundMethod"),
            };
            let cls = match self.class_of(&bm_recv) {
                Value::Class(c) => c,
                _ => return Err(self.trap(RubyError::TypeError {
                    msg: "cannot unbind method on a value without a class".into(),
                })),
            };
            self.maybe_gc();
            self.check_alloc()?;
            let id = self.heap.alloc(HeapObj::UnboundMethod { class: cls, name_id: bm_name_id });
            self.stack.push(Value::UnboundMethod(id));
            return Ok(());
        }
        // `ubm.bind(obj)` — reconstitute a BoundMethod, checking
        // that `obj` is_a? the captured class. Raises TypeError on
        // mismatch, matching CRuby.
        if let Value::UnboundMethod(uid) = &recv && &*name == "bind" && args.len() == 1 {
            let (cap_class, cap_name_id) = match self.heap.get(*uid) {
                HeapObj::UnboundMethod { class, name_id } => (class.clone(), *name_id),
                _ => panic!("ICE: UnboundMethod slot holds non-UnboundMethod"),
            };
            let mut args = args;
            let target = args.swap_remove(0);
            let target_class = match self.class_of(&target) {
                Value::Class(c) => c,
                _ => return Err(self.trap(RubyError::TypeError {
                    msg: format!("bind argument must have a class (got {})", target.type_name()),
                })),
            };
            // Kernel is the universally-bindable sentinel — CRuby
            // models it as a Module included in Object, so every
            // value is_a Kernel. We don't model Modules; skipping
            // the is_a check here gives `Kernel.instance_method(:foo)
            //   .bind(any_value)` the same shape without forcing a
            // synthetic Kernel-ancestor onto every primitive class.
            if cap_class.name != "Kernel"
                && !super::class_is_a(&target_class, &cap_class) {
                return Err(self.trap(RubyError::TypeError {
                    msg: format!(
                        "bind argument must be an instance of {} (got {})",
                        cap_class.name, target_class.name,
                    ),
                }));
            }
            // GC rooting: `target` came from `args.swap_remove(0)`,
            // which itself was drained from the operand stack at the
            // top of `do_call`. It now lives only in this Rust local
            // — not in `self.stack`, not in any frame's locals. The
            // `maybe_gc` below would otherwise sweep its heap slot
            // (Greeter.new in `kernel_instance_method.rb` under
            // STRESS_GC=1), and the BoundMethod's `recv` would point
            // at a Dead slot. Same fix shape as `Object#method` and
            // `invoke_block` rest-slot in commit 86db73d.
            let mut g = crate::vm::PinGuard::new(self);
            g.pin(target.clone());
            g.vm.maybe_gc();
            g.vm.check_alloc()?;
            let id = g.vm.heap.alloc(HeapObj::BoundMethod { recv: target, name_id: cap_name_id });
            g.vm.stack.push(Value::BoundMethod(id));
            return Ok(());
        }
        // `m.to_proc` — explicit conversion to a Proc. Equivalent
        // to the implicit `&m` coercion: routes through the same
        // `coerce_bound_method_to_block` forwarder so calling the
        // resulting Proc splats its args back into `bm.call(...)`.
        if let Value::BoundMethod(bid) = &recv
            && &*name == "to_proc" && args.is_empty() {
                let bm_id = *bid;
                let id = self.coerce_bound_method_to_block(bm_id)?;
                self.stack.push(Value::Block(id));
                return Ok(());
            }
        // `m.curry` / `m.curry(n)` — host-side partial application.
        // Returns a CurriedProc that gathers args across successive
        // `.call` invocations until `target_arity` is reached, then
        // invokes the underlying with the full arg list. `class_of`
        // reports CurriedProc as `Proc`, matching CRuby.
        if matches!(&recv, Value::BoundMethod(_) | Value::Block(_))
            && &*name == "curry" && args.len() <= 1 {
                let target_arity: u16 = if let Some(Value::Int(n)) = args.first() {
                    if *n < 0 {
                        return Err(self.trap(RubyError::ArgumentError {
                            msg: format!("negative arity for curry ({})", n),
                        }));
                    }
                    if *n > u16::MAX as i64 {
                        return Err(self.trap(RubyError::ArgumentError {
                            msg: format!("curry arity out of range ({})", n),
                        }));
                    }
                    *n as u16
                } else if let Value::BoundMethod(bid) = &recv {
                    let (bm_recv, m_name_id) = {
                        let (r, n) = self.heap.bound_method(*bid);
                        (r.clone(), n)
                    };
                    let class = match self.class_of(&bm_recv) {
                        Value::Class(c) => c,
                        _ => return Err(self.trap(RubyError::TypeError {
                            msg: "Method receiver has no resolvable class".into(),
                        })),
                    };
                    match self.lookup_method_uncached(&class, m_name_id) {
                        Some(m) => self.protos[m.proto_idx].n_required_positional,
                        None => return Err(self.trap(RubyError::ArgumentError {
                            msg: "cannot curry a method with unknown arity (builtin)".into(),
                        })),
                    }
                } else if let Value::Block(bid) = &recv {
                    // Proc#curry — derive arity from the underlying
                    // proto's required-positional count. Rest / kw
                    // are not supported as auto-arity for curry; user
                    // can still pass an explicit arity hint above.
                    let bh = self.heap.block(*bid);
                    let proto = &self.protos[bh.proto_idx];
                    if bh.rest_slot.is_some() && proto.n_required_positional == 0 {
                        return Err(self.trap(RubyError::ArgumentError {
                            msg: "cannot curry a proc with only rest params (pass explicit arity)".into(),
                        }));
                    }
                    proto.n_required_positional
                } else {
                    unreachable!()
                };
                // Pin `recv` (the underlying BoundMethod / Proc):
                // it was popped from the operand stack by do_call, so
                // it has no GC root by the time maybe_gc fires. Same
                // root-hole shape as the BoundMethod-coerce-to-Block
                // fix in PR #45 (5874798 / 50867c5).
                let mut g = crate::vm::PinGuard::new(self);
                g.pin(recv.clone());
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let id = g.vm.heap.alloc(HeapObj::CurriedProc {
                    underlying: recv.clone(),
                    gathered: Vec::new(),
                    target_arity,
                });
                g.vm.stack.push(Value::CurriedProc(id));
                return Ok(());
            }
        // `cp.call(args)` — append to gathered; invoke if arity hit,
        // else return a new CurriedProc carrying the appended state.
        if let Value::CurriedProc(cid) = &recv
            && matches!(&*name, "call" | "[]" | "()") {
                let (underlying, gathered, arity) = {
                    let (u, g, a) = self.heap.curried_proc(*cid);
                    (u.clone(), g.clone(), a)
                };
                let mut combined = gathered;
                combined.extend(args);
                if combined.len() >= arity as usize {
                    let argc = combined.len();
                    self.stack.push(underlying);
                    for a in combined { self.stack.push(a); }
                    let call_sym = self.interner.intern("call");
                    return self.do_call(call_sym, argc, false, u16::MAX);
                }
                // Same pin-the-underlying pattern as the curry-on-Method
                // branch above. `combined` may also contain heap-typed
                // arg values that are only held in this Rust-local Vec;
                // pinning the underlying alone is enough because the
                // mark phase walks CurriedProc's contents only after
                // alloc — but the new alloc's reading the SAME Vec, so
                // we need both pinned across the maybe_gc call.
                let mut g = crate::vm::PinGuard::new(self);
                g.pin(underlying.clone());
                for v in &combined { g.pin(v.clone()); }
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let id = g.vm.heap.alloc(HeapObj::CurriedProc {
                    underlying,
                    gathered: combined,
                    target_arity: arity,
                });
                g.vm.stack.push(Value::CurriedProc(id));
                return Ok(());
            }
        // `m >> other` / `m << other` — function composition.
        // `(m >> g).(x) == g.(m.(x))`; `(m << g).(x) == m.(g.(x))`.
        // Both sides must be callable — BoundMethod or Block. The
        // result is a Block (Proc) that splats `*args` through the
        // chain in the right order.
        if matches!(&recv, Value::BoundMethod(_) | Value::Block(_))
            && matches!(&*name, ">>" | "<<") && args.len() == 1 {
                let mut args = args;
                let other = args.swap_remove(0);
                if !matches!(&other, Value::BoundMethod(_) | Value::Block(_)) {
                    return Err(self.trap(RubyError::TypeError {
                        msg: format!(
                            "compose argument must be a Method or Proc (got {})",
                            other.type_name(),
                        ),
                    }));
                }
                let (outer, inner) = if &*name == ">>" {
                    (other, recv)
                } else {
                    (recv, other)
                };
                let id = self.coerce_compose_to_block(outer, inner)?;
                self.stack.push(Value::Block(id));
                return Ok(());
            }
        // `m.hash` — Integer hash derived from receiver identity
        // (ObjId / value / Rc-ptr address) + name_id. Two
        // BoundMethods compared equal under `Method#==` must
        // collide; that's the only invariant CRuby promises. The
        // mix below is wrapping_add + wrapping_mul to be cheap
        // and avoid raising.
        if matches!(&recv, Value::BoundMethod(_) | Value::UnboundMethod(_))
            && &*name == "hash" && args.is_empty() {
                let h: i64 = match &recv {
                    Value::BoundMethod(bid) => {
                        let (r, n) = self.heap.bound_method(*bid);
                        let recv_h = method_recv_hash(r);
                        recv_h.wrapping_mul(0x9E3779B1).wrapping_add(n.0 as i64)
                    }
                    Value::UnboundMethod(uid) => {
                        let (cls, n) = self.heap.unbound_method(*uid);
                        let cls_h = std::rc::Rc::as_ptr(&cls) as i64;
                        cls_h.wrapping_mul(0x9E3779B1).wrapping_add(n.0 as i64)
                    }
                    _ => unreachable!(),
                };
                self.stack.push(Value::Int(h));
                return Ok(());
            }
        // `m.source_location` — `[filename, lineno]` for user-
        // defined methods; `nil` for builtins (no Method record
        // in any class). Lineno is computed from the proto's
        // first op_span via the Vm-side `sources` mirror; falls
        // back to 0 if the source text isn't available (rare —
        // synthesised protos for forwarders / preamble eval).
        if matches!(&recv, Value::BoundMethod(_) | Value::UnboundMethod(_))
            && &*name == "source_location" && args.is_empty() {
                let (class, m_name_id) = match &recv {
                    Value::BoundMethod(bid) => {
                        let (r, n) = self.heap.bound_method(*bid);
                        let r = r.clone();
                        let cls = match self.class_of(&r) {
                            Value::Class(c) => c,
                            _ => { self.stack.push(Value::Nil); return Ok(()); }
                        };
                        (cls, n)
                    }
                    Value::UnboundMethod(uid) => self.heap.unbound_method(*uid),
                    _ => unreachable!(),
                };
                let m = match self.lookup_method_uncached(&class, m_name_id) {
                    Some(m) => m,
                    None => { self.stack.push(Value::Nil); return Ok(()); }
                };
                let proto = &self.protos[m.proto_idx];
                let filename = proto.filename.clone();
                let first_offset = proto.op_spans.first().map(|s| s.byte_offset).unwrap_or(0);
                let line: u32 = self.sources.get(&*filename)
                    .map(|src| crate::error::line_col(src, first_offset).0)
                    .unwrap_or(0);
                let filename_str = Value::new_str(filename.to_string());
                self.maybe_gc();
                self.check_alloc()?;
                let id = self.heap.alloc(HeapObj::Array(vec![filename_str, Value::Int(line as i64)]));
                self.stack.push(Value::Array(id));
                return Ok(());
            }
        // `m.owner` — the class that defined the resolved Method
        // (CRuby's `Method#owner` / `UnboundMethod#owner`). Walks
        // the ancestor chain to find where the method actually
        // lives; falls back to the captured class for builtins
        // (whose primitive_call backing has no Method record).
        //
        // `m.receiver` — the captured recv on a BoundMethod.
        // UnboundMethod#receiver raises NoMethodError, matching
        // CRuby (it has no receiver to give).
        if matches!(&recv, Value::BoundMethod(_) | Value::UnboundMethod(_))
            && matches!(&*name, "owner" | "receiver") && args.is_empty() {
                if &*name == "receiver" {
                    return match &recv {
                        Value::BoundMethod(bid) => {
                            let (r, _) = self.heap.bound_method(*bid);
                            let r = r.clone();
                            self.stack.push(r);
                            Ok(())
                        }
                        Value::UnboundMethod(_) => Err(self.trap(RubyError::NoMethodError {
                            method: "receiver".into(),
                            recv_type: "UnboundMethod",
                        })),
                        _ => unreachable!(),
                    };
                }
                // owner: resolve Method through lookup; prefer its
                // defining_class.upgrade() over the captured class.
                let (cap_class, m_name_id) = match &recv {
                    Value::BoundMethod(bid) => {
                        let (r, n) = self.heap.bound_method(*bid);
                        let r = r.clone();
                        let cls = match self.class_of(&r) {
                            Value::Class(c) => c,
                            _ => return Err(self.trap(RubyError::TypeError {
                                msg: "Method receiver has no resolvable class".into(),
                            })),
                        };
                        (cls, n)
                    }
                    Value::UnboundMethod(uid) => self.heap.unbound_method(*uid),
                    _ => unreachable!(),
                };
                let owner = match self.lookup_method_uncached(&cap_class, m_name_id) {
                    Some(m) => m.defining_class.as_ref()
                        .and_then(|w| w.upgrade())
                        .unwrap_or_else(|| cap_class.clone()),
                    None => cap_class.clone(),
                };
                self.stack.push(Value::Class(owner));
                return Ok(());
            }
        // `m.arity` / `m.parameters` — Method introspection. Walks
        // the captured class chain to find the user-defined Method;
        // if absent (builtin / primitive_call backed), returns
        // CRuby's "fully varadic" signature: arity = -1,
        // parameters = `[[:rest]]`. Same shape for BoundMethod and
        // UnboundMethod.
        if matches!(&recv, Value::BoundMethod(_) | Value::UnboundMethod(_))
            && matches!(&*name, "arity" | "parameters") && args.is_empty() {
                let (class, m_name_id) = match &recv {
                    Value::BoundMethod(bid) => {
                        let (bm_recv, nid) = {
                            let (r, n) = self.heap.bound_method(*bid);
                            (r.clone(), n)
                        };
                        let cls = match self.class_of(&bm_recv) {
                            Value::Class(c) => c,
                            _ => return Err(self.trap(RubyError::TypeError {
                                msg: "Method receiver has no resolvable class".into(),
                            })),
                        };
                        (cls, nid)
                    }
                    Value::UnboundMethod(uid) => self.heap.unbound_method(*uid),
                    _ => unreachable!(),
                };
                let m_opt = self.lookup_method_uncached(&class, m_name_id);
                let (arity, params_info) = match m_opt {
                    Some(m) => {
                        let proto = &self.protos[m.proto_idx];
                        let n_req_pos = proto.n_required_positional as usize;
                        let rest_count = proto.rest_param.is_some() as usize;
                        let kw_count = proto.kw_param_defaults.len();
                        let kw_rest_count = proto.kw_rest_param.is_some() as usize;
                        let positional_total = proto.params.len()
                            .saturating_sub(rest_count + kw_count + kw_rest_count);
                        let n_opt_pos = positional_total.saturating_sub(n_req_pos);
                        let n_req_kw = proto.kw_param_defaults.iter().filter(|d| d.is_none()).count();
                        let n_opt_kw = proto.kw_param_defaults.iter().filter(|d| d.is_some()).count();
                        // CRuby's arity rule: any *required* keyword
                        // adds 1 to the mandatory count; the kwargs
                        // bundle is then treated as a single
                        // mandatory arg (so the signature is "fully
                        // specified" if there's no opt-pos / rest).
                        // If there are no required kwargs but some
                        // optional/kw_rest are present, the bundle
                        // is treated as a single OPTIONAL arg —
                        // arity goes negative.
                        let req_kw_present = n_req_kw > 0;
                        let effective_req = n_req_pos + req_kw_present as usize;
                        let has_pos_optional = n_opt_pos > 0 || rest_count > 0;
                        let has_kw_optional = !req_kw_present && (n_opt_kw > 0 || kw_rest_count > 0);
                        let arity: i64 = if has_pos_optional || has_kw_optional {
                            -((effective_req + 1) as i64)
                        } else {
                            effective_req as i64
                        };
                        let mut params: Vec<(&'static str, Option<String>)> = Vec::new();
                        for i in 0..n_req_pos {
                            params.push(("req", Some(proto.params[i].clone())));
                        }
                        for i in n_req_pos..positional_total {
                            params.push(("opt", Some(proto.params[i].clone())));
                        }
                        if let Some(rname) = &proto.rest_param {
                            let n = if rname.is_empty() { None } else { Some(rname.clone()) };
                            params.push(("rest", n));
                        }
                        let kw_name_start = positional_total + rest_count;
                        for (i, default) in proto.kw_param_defaults.iter().enumerate() {
                            let kind = if default.is_none() { "keyreq" } else { "key" };
                            params.push((kind, Some(proto.params[kw_name_start + i].clone())));
                        }
                        if let Some(krname) = &proto.kw_rest_param {
                            let n = if krname == "__kw_rest_anon" { None } else { Some(krname.clone()) };
                            params.push(("keyrest", n));
                        }
                        (arity, params)
                    }
                    None => (-1i64, vec![("rest", None)]),
                };
                if &*name == "arity" {
                    self.stack.push(Value::Int(arity));
                    return Ok(());
                }
                // Build [[kind_sym, name_sym?], ...] array. Anonymous
                // rest / kw_rest yields a single-element pair, matching
                // CRuby's `[[:rest]]` / `[[:keyrest]]`.
                //
                // PinGuard across the whole loop so the inner-pair
                // ObjIds in `outer` survive every maybe_gc — without
                // this, under STRESS_GC each iteration's pair slot
                // gets swept (no GC root: `outer` is a Rust-local
                // Vec), the next alloc reuses it, and the final
                // `heap.alloc(HeapObj::Array(outer))` can land on the
                // same recycled slot — yielding a self-referencing
                // Array whose `.inspect` recurses to stack overflow.
                let mut g = crate::vm::PinGuard::new(self);
                let mut outer: Vec<Value> = Vec::with_capacity(params_info.len());
                for (kind, name_opt) in params_info {
                    let kind_sym = g.vm.interner.intern(kind);
                    let mut pair = vec![Value::Sym(kind_sym)];
                    if let Some(n) = name_opt {
                        let nsym = g.vm.interner.intern(&n);
                        pair.push(Value::Sym(nsym));
                    }
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    let pid = g.vm.heap.alloc(HeapObj::Array(pair));
                    g.pin(Value::Array(pid));
                    outer.push(Value::Array(pid));
                }
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let aid = g.vm.heap.alloc(HeapObj::Array(outer));
                g.vm.stack.push(Value::Array(aid));
                return Ok(());
            }
        if let Value::BoundMethod(bid) = &recv
            && matches!(&*name, "call" | "[]" | "()") {
                let (bm_recv, bm_name_id) = match self.heap.get(*bid) {
                    HeapObj::BoundMethod { recv, name_id } => (recv.clone(), *name_id),
                    _ => panic!("ICE: BoundMethod slot holds non-BoundMethod"),
                };
                let argc = args.len();
                self.stack.push(bm_recv);
                for a in args {
                    self.stack.push(a);
                }
                return self.do_call(
                    bm_name_id, argc,
                    /* no_recv = */ false,
                    /* cache_id = */ u16::MAX,
                );
            }
        if let Value::Class(target) = &recv
            && (&*name == "include" || &*name == "extend") && !args.is_empty() {
                // Explicit-receiver form: `MyClass.include(Mod)`.
                // Same chain-push semantics as the no-receiver
                // form above — see that comment for the rationale.
                for a in &args {
                    let src = match a {
                        Value::Class(c) => c.clone(),
                        _ => return Err(self.trap(RubyError::TypeError {
                            msg: format!(
                                "wrong argument type {} (expected Module)",
                                a.type_name(),
                            ),
                        })),
                    };
                    let mut chain = target.includes.borrow_mut();
                    if !chain.iter().any(|c| Rc::ptr_eq(c, &src)) {
                        chain.insert(0, src);
                    }
                }
                self.method_gen = self.method_gen.wrapping_add(1);
                self.stack.push(recv.clone());
                return Ok(());
            }
        // Universal class predicates: `is_a?` / `kind_of?` walk
        // the ancestor chain (own class + includes + superclass);
        // `instance_of?` is exact-class only. CRuby exposes both
        // on `Object`, so they apply to every receiver — for
        // primitives (Int / Str / Sym / ...) we resolve their
        // class via `class_of`.
        if matches!(&*name, "is_a?" | "kind_of?" | "instance_of?") && args.len() == 1
            && let Value::Class(target) = &args[0] {
                let recv_class_v = self.class_of(&recv);
                let recv_class = if let Value::Class(c) = recv_class_v { c } else {
                    self.stack.push(Value::Bool(false));
                    return Ok(());
                };
                let result = if &*name == "instance_of?" {
                    Rc::ptr_eq(&recv_class, target)
                } else {
                    super::class_is_a(&recv_class, target)
                };
                self.stack.push(Value::Bool(result));
                return Ok(());
            }
        // Class introspection: `ancestors` / `include?`. Walks the
        // chain via `class_is_a` (covers superclass + includes).
        // Returned Array is freshly allocated, so the path needs
        // heap access — kept here rather than in `primitive_call`.
        if let Value::Class(cls) = &recv {
            match (&*name, args.as_slice()) {
                ("ancestors", []) => {
                    let mut chain: Vec<Value> = Vec::new();
                    let mut current = cls.clone();
                    loop {
                        chain.push(Value::Class(current.clone()));
                        for inc in current.includes.borrow().iter() {
                            chain.push(Value::Class(inc.clone()));
                        }
                        let parent = current.superclass.borrow().clone();
                        match parent {
                            Some(p) => current = p,
                            None => break,
                        }
                    }
                    self.maybe_gc();
                    self.check_alloc()?;
                    let id = self.heap.alloc(HeapObj::Array(chain));
                    self.stack.push(Value::Array(id));
                    return Ok(());
                }
                ("include?", [Value::Class(m)]) => {
                    let included = super::class_is_a(cls, m);
                    self.stack.push(Value::Bool(included));
                    return Ok(());
                }
                // `Class#instance_method(:sym)` — direct UnboundMethod
                // construction. Walks the ancestor chain via
                // `lookup_method_uncached`; NameError if the method
                // isn't defined anywhere in the chain. Captures the
                // receiver class (not the *defining* class) — the
                // same approximation as `Method#unbind`.
                //
                // Primitive-class special case: classes whose
                // instances are backed by non-Object `Value`
                // variants (Integer / Float / String / Symbol /
                // Array / Hash / Range / Regexp / Proc / Method /
                // UnboundMethod / TrueClass / FalseClass / NilClass)
                // have no entries in their `methods` table — their
                // dispatch happens through `primitive_call` /
                // `numeric_call` / etc. instead of user-Method
                // records. `lookup_method_uncached` always returns
                // None for them.
                //
                // The CRuby-faithful answer here is "look up the
                // built-in dispatch table and synthesise an
                // UnboundMethod with real arity / parameters."
                // The Tier 1 pragmatic answer is "produce a
                // synthetic UnboundMethod that exists for any
                // method name on these classes; the downstream
                // `arity` / `parameters` arms already return the
                // sensible fallback (arity = -1,
                // parameters = [[:rest]]) when the Method record
                // is absent." That's enough to let pure-Ruby gem
                // helpers (msgpack's `lib/msgpack/bigint.rb`'s
                // `if Integer.instance_method(:[]).arity != 1`
                // version detect) load cleanly. User classes
                // still raise NameError for unknown methods —
                // matching CRuby behaviour for the case that
                // matters most (typo detection in user code).
                // `Class#method_defined?(:sym)` — presence check
                // for an instance method anywhere on the class's
                // own table or its ancestor chain (own +
                // `include`-d modules + superclass). CRuby's
                // 2-arg form (`method_defined?(:foo, false)`)
                // excludes the private-method tail; we don't
                // model visibility-aware skipping, so we
                // implement the canonical 1-arg shape and a
                // permissive 2-arg form whose second arg is
                // accepted-and-ignored. Primitive classes —
                // those whose instances are non-Object `Value`
                // variants (Integer / Float / String / Symbol /
                // Array / Hash / Range / Regexp / Proc / Method
                // / UnboundMethod / TrueClass / FalseClass /
                // NilClass) — return `true` for any name so
                // pure-Ruby helpers that probe these classes
                // (msgpack-ruby's `lib/msgpack/symbol.rb`'s
                // `if method_defined?(:name)` Ruby-2.7+ version
                // detect) don't trip on a hard-false where
                // CRuby would say yes. Matches the same shape as
                // the `instance_method` arm above.
                ("method_defined?", [Value::Sym(sid)])
                | ("method_defined?", [Value::Sym(sid), _]) => {
                    let answer = class_method_defined(self, cls, *sid);
                    self.stack.push(Value::Bool(answer));
                    return Ok(());
                }
                ("method_defined?", [Value::Str(s)])
                | ("method_defined?", [Value::Str(s), _]) => {
                    let sid = self.interner.intern(&s.to_string_lossy());
                    let answer = class_method_defined(self, cls, sid);
                    self.stack.push(Value::Bool(answer));
                    return Ok(());
                }
                // `Class#undef_method(:name)` — CRuby removes
                // the method from the class so subsequent calls
                // raise NoMethodError. The Tier 1 subset doesn't
                // model "undefined-by-name-but-not-by-table-
                // delete", and the typical use case is purely
                // defensive (`undef_method :dup` on cext-owned
                // classes to discourage scripts from cloning a
                // pointer-backed object). No-op for now — matches
                // the same conservative shape `Class#private` /
                // `#public` take when called with arguments
                // (visibility flag isn't propagated). Documented
                // divergence; lets msgpack-ruby `lib/msgpack/
                // packer.rb` / `buffer.rb` / `unpacker.rb` load
                // cleanly (each calls `undef_method :dup` /
                // `:clone` at class-body top level). Accepts
                // Symbol/String args; variadic per CRuby.
                ("undef_method", _) => {
                    // Return the class itself (CRuby returns the
                    // Module the call was made on) so chain-style
                    // uses (`MyClass.undef_method(:foo).new`)
                    // remain syntactically valid; not a primary
                    // use case but cheap to preserve.
                    self.stack.push(Value::Class(cls.clone()));
                    return Ok(());
                }
                ("instance_method", [Value::Sym(sid)]) => {
                    let found = self.lookup_method_uncached(cls, *sid).is_some();
                    if !found && !is_primitive_class_name(&cls.name) {
                        let mname = self.interner.resolve(*sid).to_string();
                        return Err(self.trap(RubyError::NameError {
                            msg: format!("undefined method '{}' for class '{}'", mname, cls.name),
                        }));
                    }
                    let cls_owned = cls.clone();
                    self.maybe_gc();
                    self.check_alloc()?;
                    let id = self.heap.alloc(HeapObj::UnboundMethod { class: cls_owned, name_id: *sid });
                    self.stack.push(Value::UnboundMethod(id));
                    return Ok(());
                }
                _ => {}
            }
        }
        if let Some(v) = self.collection_call(&recv, &name, &args)? {
            self.stack.push(v);
            return Ok(());
        }
        // `obj.methods` — Array of Symbols of every method the
        // receiver can dispatch. For user instances walks the
        // class chain (own + includes + superclass); for other
        // shapes returns an empty Array (the subset doesn't
        // expose Kernel-level methods individually). De-dups by
        // SymId, sorted by interner string order for determinism.
        if &*name == "methods" && args.is_empty() {
            let mut names: Vec<crate::intern::SymId> = Vec::new();
            if let Value::Object(id) = &recv {
                let cls = self.heap.class_of(*id);
                let mut visited: Vec<*const crate::value::Class> = Vec::new();
                fn walk(
                    c: &std::rc::Rc<crate::value::Class>,
                    out: &mut Vec<crate::intern::SymId>,
                    visited: &mut Vec<*const crate::value::Class>,
                ) {
                    let ptr = std::rc::Rc::as_ptr(c);
                    if visited.contains(&ptr) { return; }
                    visited.push(ptr);
                    for k in c.methods.borrow().keys() {
                        if !out.contains(k) { out.push(*k); }
                    }
                    for inc in c.includes.borrow().iter() {
                        walk(inc, out, visited);
                    }
                    if let Some(sup) = c.superclass.borrow().clone() {
                        walk(&sup, out, visited);
                    }
                }
                walk(&cls, &mut names, &mut visited);
                names.sort_by(|a, b| {
                    self.interner.resolve(*a).cmp(self.interner.resolve(*b))
                });
            }
            let elems: Vec<Value> = names.into_iter().map(Value::Sym).collect();
            self.maybe_gc();
            self.check_alloc()?;
            let id = self.heap.alloc(HeapObj::Array(elems));
            self.stack.push(Value::Array(id));
            return Ok(());
        }
        // `obj.instance_variables` — Array of Symbols (with `@`
        // prefix). Only Value::Object instances actually carry
        // ivars; other shapes get an empty Array.
        if &*name == "instance_variables" && args.is_empty() {
            let mut names: Vec<Value> = Vec::new();
            if let Value::Object(id) = &recv {
                let ivar_ids: Vec<crate::intern::SymId> = {
                    if let crate::heap::HeapObj::Instance(inst) = self.heap.get(*id) {
                        inst.ivars.keys().copied().collect()
                    } else {
                        Vec::new()
                    }
                };
                let mut decorated: Vec<(String, crate::intern::SymId)> = ivar_ids.into_iter()
                    .map(|s| {
                        let raw = self.interner.resolve(s).to_string();
                        // Internal interner key includes the `@`
                        // prefix already (matches how parser interns
                        // ivar names). If not, prepend.
                        let key = if raw.starts_with('@') { raw } else { format!("@{}", raw) };
                        (key, s)
                    })
                    .collect();
                decorated.sort_by(|a, b| a.0.cmp(&b.0));
                for (key, _) in decorated {
                    let sid = self.interner.intern(&key);
                    names.push(Value::Sym(sid));
                }
            }
            self.maybe_gc();
            self.check_alloc()?;
            let id = self.heap.alloc(HeapObj::Array(names));
            self.stack.push(Value::Array(id));
            return Ok(());
        }
        // `Integer#digits([base])` — LSB-first digit Array. Default
        // base 10; custom base must be >= 2. Negative receivers
        // raise (CRuby raises Math::DomainError; subset uses
        // ArgumentError since Math::DomainError isn't modelled).
        if let Value::Int(n) = &recv && &*name == "digits" && args.len() <= 1 {
            let base: i64 = match args.first() {
                None => 10,
                Some(Value::Int(b)) => *b,
                Some(other) => return Err(self.trap(RubyError::TypeError {
                    msg: format!("no implicit conversion of {} into Integer", other.type_name()),
                })),
            };
            if base < 2 {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!("invalid radix {}", base),
                }));
            }
            if *n < 0 {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: "numerical argument is out of domain - \"digits\"".into(),
                }));
            }
            let mut elems: Vec<Value> = Vec::new();
            let mut m = *n;
            if m == 0 {
                elems.push(Value::Int(0));
            } else {
                while m > 0 {
                    elems.push(Value::Int(m % base));
                    m /= base;
                }
            }
            self.maybe_gc();
            self.check_alloc()?;
            let id = self.heap.alloc(HeapObj::Array(elems));
            self.stack.push(Value::Array(id));
            return Ok(());
        }
        // `Object#equal?` — identity comparison. For heap-managed
        // receivers, same `ObjId`; for inline values, same content.
        // CRuby never overrides this on subclasses, so we always
        // intercept (above class-lookup would be redundant work).
        if &*name == "equal?" && args.len() == 1 {
            let same = match (&recv, &args[0]) {
                (Value::Object(a), Value::Object(b)) => a == b,
                (Value::Array(a), Value::Array(b)) => a == b,
                (Value::Hash(a), Value::Hash(b)) => a == b,
                (Value::Range(a), Value::Range(b)) => a == b,
                (Value::Block(a), Value::Block(b)) => a == b,
                (Value::Class(a), Value::Class(b)) => Rc::ptr_eq(a, b),
                // String is now Rc-shared and identity-bearing
                // (frozen flag, aliasing). `equal?` should reflect
                // Rc-pointer identity, not content equality.
                (Value::Str(a), Value::Str(b)) => Rc::ptr_eq(a, b),
                // Immediates (Int, Float, Sym, Bool, Nil) — fall
                // back on ruby_eq (value equality).
                _ => recv.ruby_eq(&args[0], &self.heap),
            };
            self.stack.push(Value::Bool(same));
            return Ok(());
        }
        // `Method#==` / `UnboundMethod#==` — intercept before the
        // universal `==` fallback (which has no arm for these and
        // would return `false`).
        //
        // BoundMethod: same name_id AND receiver identity. Heap-
        // backed recvs compare by ObjId / Rc-pointer; primitives
        // (Int / Sym / Bool / ...) compare by value. This matches
        // CRuby, where `s1.method(:length) == s2.method(:length)`
        // is `false` for distinct String instances but `true` for
        // the same Integer literal.
        //
        // UnboundMethod: lookup both classes' Method records via
        // `lookup_method_uncached` (walks ancestor chain) and
        // compare by Rc-pointer. Two UnboundMethods that resolve
        // to the same underlying definition — e.g., a parent's
        // method inherited by a subclass — are equal, matching
        // CRuby's `C.instance_method(:foo) == D.instance_method(:foo)`.
        if args.len() == 1 && &*name == "=="
            && matches!(&recv, Value::BoundMethod(_) | Value::UnboundMethod(_)) {
                let other = &args[0];
                let result = match (&recv, other) {
                    (Value::BoundMethod(a), Value::BoundMethod(b)) => {
                        let (ra, na) = self.heap.bound_method(*a);
                        let ra = ra.clone();
                        let (rb, nb) = self.heap.bound_method(*b);
                        let rb = rb.clone();
                        na == nb && method_recv_identity(&ra, &rb)
                    }
                    (Value::UnboundMethod(a), Value::UnboundMethod(b)) => {
                        let (ca, na) = self.heap.unbound_method(*a);
                        let (cb, nb) = self.heap.unbound_method(*b);
                        let ma = self.lookup_method_uncached(&ca, na);
                        let mb = self.lookup_method_uncached(&cb, nb);
                        match (ma, mb) {
                            (Some(x), Some(y)) => Rc::ptr_eq(&x, &y),
                            _ => na == nb && Rc::ptr_eq(&ca, &cb),
                        }
                    }
                    _ => false,
                };
                self.stack.push(Value::Bool(result));
                return Ok(());
            }
        // `Object#==` / `Object#!=` cross-type fallback. The
        // per-type primitive arms (`String == String`,
        // `Sym == Sym`, `Class == Class`, etc.) all fired earlier
        // in this dispatch. Anything that reaches here is a
        // cross-type comparison (`"x" == nil`, `nil == :foo`,
        // `[] == ""`) — those return `false` in CRuby, not
        // NoMethodError. Same-type comparisons that we don't
        // have per-type arms for (e.g. `Array == Array`) get
        // value-equality via `ruby_eq`. Universal fallback —
        // never raises — so it must go before NoMethodError.
        if args.len() == 1 && (&*name == "==" || &*name == "!=") {
            let eq = recv.ruby_eq(&args[0], &self.heap);
            let result = if &*name == "==" { eq } else { !eq };
            self.stack.push(Value::Bool(result));
            return Ok(());
        }
        // `===` case-equality. Used by `case/when` desugaring.
        // Per-type semantics:
        //   Range#=== → include? (numeric containment)
        //   Class#=== → instance-of (walks ancestor chain)
        //   everything else → `==` value equality
        // User classes can override `===` via class-method
        // lookup, which fires above this fallback (no shadowing
        // needed since the universal check is the last resort).
        if &*name == "===" && args.len() == 1 {
            let arg = &args[0];
            let result = match &recv {
                Value::Range(rid) => {
                    // Generic numeric containment: coerce both
                    // bounds and the arg to Float so Int/Float
                    // mixes (5 in 1..10, 5.0 in 0..10, 5 in 0.0..10.0)
                    // all work. Strings / Symbols compare
                    // lexicographically — handled below.
                    let r = self.heap.range(*rid);
                    fn to_f64(v: &Value) -> Option<f64> {
                        match v {
                            Value::Int(n) => Some(*n as f64),
                            Value::Float(f) => Some(*f),
                            _ => None,
                        }
                    }
                    let excl = r.exclusive;
                    
                    match (to_f64(&r.begin), to_f64(&r.end), to_f64(arg)) {
                        (Some(b), Some(e), Some(v)) => {
                            if excl { v >= b && v < e }
                            else { v >= b && v <= e }
                        }
                        _ => {
                            // Non-numeric: fall back to lexicographic
                            // compare using value_cmp_v if both bounds
                            // and the arg are the same comparable type.
                            let b = &r.begin; let e = &r.end;
                            let ge_lo = value_cmp_v(arg, b, &self.interner)
                                .map(|o| o != std::cmp::Ordering::Less)
                                .unwrap_or(false);
                            let cmp_hi = value_cmp_v(arg, e, &self.interner);
                            let le_hi = match cmp_hi {
                                Some(o) => if excl { o == std::cmp::Ordering::Less }
                                           else { o != std::cmp::Ordering::Greater },
                                None => false,
                            };
                            ge_lo && le_hi
                        }
                    }
                }
                Value::Class(target) => {
                    // Walk the argument's class chain looking for
                    // an Rc-identical match with `target`. For
                    // built-in receivers, look up the stub class
                    // by interned type name.
                    let start: Option<Rc<Class>> = match arg {
                        Value::Object(id) => Some(self.heap.class_of(*id)),
                        _ => {
                            let class_val = self.class_of(arg);
                            if let Value::Class(c) = class_val { Some(c) } else { None }
                        }
                    };
                    let mut cur = start;
                    let mut hit = false;
                    while let Some(cls) = cur {
                        if Rc::ptr_eq(&cls, target) { hit = true; break; }
                        cur = cls.superclass.borrow().clone();
                    }
                    hit
                }
                #[cfg(feature = "regex")]
                Value::Regex(re) => match arg {
                    // CRuby: `Regexp#===` (used by `case/when`) sets
                    // `$~`/`$1`.. on hit and clears them on miss,
                    // just like `=~`/`String#match`. Switch from
                    // `is_match` to `captures` so the side-channel
                    // sees the same view through every entry point.
                    Value::Str(s) => s.with_str_lossy(|s| match re.captures(s) {
                        Some(caps) => {
                            let m0 = caps.get(0).unwrap();
                            self.last_match = Some(crate::vm::LastMatch {
                                whole: m0.as_str().to_string(),
                                caps: (1..caps.len())
                                    .map(|i| caps.get(i).map(|m| m.as_str().to_string()))
                                    .collect(),
                            });
                            true
                        }
                        None => {
                            self.last_match = None;
                            false
                        }
                    }),
                    _ => false,
                },
                _ => recv.ruby_eq(arg, &self.heap),
            };
            self.stack.push(Value::Bool(result));
            return Ok(());
        }
        // `=~` — Regex/String matching. Returns the byte offset of
        // the first match, or nil. On a hit, populate `last_match`
        // (with captures) so `$~` and `$1`..`$N` (any positive
        // index — multi-digit forms like `$10` work too) see the
        // same match; on a miss, clear it (CRuby parity — a failed
        // `=~` wipes the prior match's globals).
        if &*name == "=~" && args.len() == 1 {
            let result = match (&recv, &args[0]) {
                #[cfg(feature = "regex")]
                (Value::Regex(re), Value::Str(s)) | (Value::Str(s), Value::Regex(re)) => {
                    let bound = s.to_string_lossy();
                    match re.captures(&bound) {
                        Some(caps) => {
                            let m0 = caps.get(0).unwrap();
                            let start = m0.start() as i64;
                            self.last_match = Some(crate::vm::LastMatch {
                                whole: m0.as_str().to_string(),
                                caps: (1..caps.len())
                                    .map(|i| caps.get(i).map(|m| m.as_str().to_string()))
                                    .collect(),
                            });
                            Value::Int(start)
                        }
                        None => {
                            self.last_match = None;
                            Value::Nil
                        }
                    }
                }
                _ => Value::Nil,
            };
            self.stack.push(result);
            return Ok(());
        }
        // `Object#<=>` fallback for `Value::Object` receivers. The
        // per-type primitive_call arms above handle every built-in
        // lhs (Int / Float / Str / Bool / Nil — Sym lives in
        // sym_primitive). When we reach here on `<=>`, the only
        // remaining lhs shape is `Value::Object` whose class
        // didn't define `<=>`. CRuby's default `Object#<=>`
        // returns `0` if the two values are identical (in our
        // model: same `ObjId`) and `nil` otherwise. User-defined
        // `<=>` on a class already fired via class-method-lookup
        // earlier, so we don't shadow.
        if &*name == "<=>" && args.len() == 1 {
            let result = match (&recv, &args[0]) {
                (Value::Object(a), Value::Object(b)) if a == b => Value::Int(0),
                _ => Value::Nil,
            };
            self.stack.push(result);
            return Ok(());
        }
        // `Object#class` — universal, no args. Returns the Class
        // associated with the receiver. For built-in types it's
        // the stub class registered by the preamble; for user
        // instances it's the instance's stored class.
        if &*name == "class" && args.is_empty() {
            let c = self.class_of(&recv);
            self.stack.push(c);
            return Ok(());
        }
        // `Object#respond_to?(name)` — pure feature detection, no
        // invocation. Goes last so user classes that override
        // `respond_to?` (we don't support that yet, but conceptually)
        // would shadow this. Accepts either a `Symbol` or a `String`
        // argument; anything else falls through to NoMethodError.
        if &*name == "respond_to?" && args.len() == 1 {
            let lookup_name: Option<SymId> = match &args[0] {
                Value::Sym(id) => Some(*id),
                Value::Str(s) => Some(self.interner.intern(&s.to_string_lossy())),
                _ => None,
            };
            if let Some(id) = lookup_name {
                let yes = self.responds_to(&recv, id);
                self.stack.push(Value::Bool(yes));
                return Ok(());
            }
        }
        if self.try_method_missing(&recv, name_id, args, None)? {
            return Ok(());
        }
        Err(self.trap(RubyError::NoMethodError {
            method: name.to_string(), recv_type: recv.type_name(),
        }))
    }



    pub(crate) fn invoke_method(&mut self, m: Rc<Method>, self_val: Value, args: Vec<Value>) -> Result<(), Trap> {
        self.invoke_method_with_block(m, self_val, args, None)
    }



    pub(crate) fn invoke_method_with_block(&mut self, m: Rc<Method>, self_val: Value, args: Vec<Value>, block: Option<ObjId>) -> Result<(), Trap> {
        // `define_method`-installed methods carry a captured Rc and
        // diverge from the normal fresh-locals path: their frame
        // *shares* `captured` with the lexical scope that created
        // the block. Writes to outer-scope locals from inside the
        // method body propagate back, matching CRuby semantics.
        if let Some(cl) = &m.closure {
            let given = args.len();
            let n_params = cl.n_params as usize;
            if given != n_params {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!("wrong number of arguments (given {}, expected {})", given, n_params),
                }));
            }
            self.check_frames()?;
            let proto_idx = m.proto_idx;
            let proto_n_locals = self.protos[proto_idx].n_locals as usize;
            let param_start = cl.param_start as usize;
            // Block params live *after* the captured frame's n_locals
            // (block locals layout inherits the parent — see ADR 0004).
            // Resize the shared Vec if a previous invocation hasn't
            // already grown it.
            {
                let mut caps = cl.captured.borrow_mut();
                let need = param_start.max(proto_n_locals);
                if caps.len() < need {
                    caps.resize(need, Value::Nil);
                }
                for (i, a) in args.into_iter().enumerate() {
                    caps[param_start + i] = a;
                }
            }
            self.frames.push(Frame {
                proto_idx,
                ip: 0,
                locals: cl.captured.clone(),
                self_val,
                base_sp: self.stack.len(),
                is_class_body: false, swap_return: None, block_arg: block, defining_class: m.defining_class.as_ref().and_then(|w| w.upgrade()), is_block: false,
                // `define_method` enforces exact arity (no
                // defaults), so all params are "given".
                n_given_positional: given as u16,
                rescues: vec![], loop_rescue_depths: vec![], loop_stack_depths: vec![],
            });
            return Ok(());
        }
        // Default-argument support (literal defaults only): a Proto
        // carries a `defaults` vec parallel to `params`. `None`
        // entries are required; `Some(v)` entries can be omitted by
        // the caller and the slot is filled from the literal at
        // invocation time. Required params always come before
        // optionals in source order, so the legal arg-count range
        // is `[required, params.len()]`.
        //
        // Rest-param (`*args`) — m.params holds the positional
        // names; the rest-name (if any) is in proto.rest_param.
        // Excess args past `params.len()` collect into an Array
        // bound to the rest slot. With a rest param there's no
        // upper bound on the arg count.
        let proto = &self.protos[m.proto_idx];
        let has_rest = proto.rest_param.is_some();
        let has_kw_rest = proto.kw_rest_param.is_some();
        let has_block_param = proto.block_param.is_some();
        let kw_count = proto.kw_param_defaults.len();
        // Layout of `m.params` tail:
        //   [...positional..., rest?, ...kw_params..., kw_rest?, block_param?]
        let positional_max = m.params.len()
            - (if has_rest { 1 } else { 0 })
            - kw_count
            - (if has_kw_rest { 1 } else { 0 })
            - (if has_block_param { 1 } else { 0 });
        let required = proto.n_required_positional as usize;
        // Pop trailing Hash arg (if present and we expect kw
        // params) — those entries become keyword bindings, not
        // positional args.
        let mut args = args;
        let kw_hash: Option<Vec<(Value, Value)>> = if kw_count > 0 || has_kw_rest {
            if let Some(Value::Hash(hid)) = args.last().cloned() {
                args.pop();
                Some(self.heap.hash(hid).clone())
            } else {
                None
            }
        } else {
            None
        };
        let given = args.len();
        let arity_ok = if has_rest {
            given >= required
        } else {
            given >= required && given <= positional_max
        };
        if !arity_ok {
            let expected = if has_rest {
                format!("{}+", required)
            } else if required == positional_max {
                format!("{}", required)
            } else {
                format!("{}..{}", required, positional_max)
            };
            return Err(self.trap(RubyError::ArgumentError {
                msg: format!("wrong number of arguments (given {}, expected {})", given, expected),
            }));
        }
        self.check_frames()?;
        let n_locals = proto.n_locals as usize;
        // Snapshot proto-derived data needed during arg binding,
        // dropping the immutable borrow on self.protos so the
        // subsequent maybe_gc / heap.alloc calls (for the rest
        // Array) can take &mut self.
        let kw_defaults_snapshot: Vec<Option<Value>> = proto.kw_param_defaults.clone();
        // Optional positional slots that the caller omitted stay
        // `Nil` here; the method body's entry prologue runs
        // `Op::JumpIfArgGiven(slot, skip)` + default-expr +
        // `Op::StoreLocal(slot)` per optional, evaluating any
        // expression (literal, prior param, constant lookup, full
        // method call). `frame.n_given_positional = positional_take`
        // is what the prologue consults to tell "caller-supplied"
        // from "left for default-eval".
        let mut locals = vec_nil(n_locals);
        // Bind up to positional_max args into positional slots; any
        // overflow flows into the rest slot as a fresh Array.
        let positional_take = given.min(positional_max);
        let mut args_iter = args.into_iter();
        for slot in locals.iter_mut().take(positional_take) {
            *slot = args_iter.next().unwrap();
        }
        if has_rest {
            // Remaining args (possibly empty) → fresh Array in the
            // rest slot.
            //
            // GC root hole guard: at this point everything we need
            // to survive `maybe_gc` lives only as Rust locals —
            // not in `self.stack`, `self.frames`, or `self.pinned`.
            // That covers:
            //   - `locals` — the not-yet-installed frame locals
            //     (already populated with positional + default args)
            //   - `rest_vec` — trailing args destined for the rest slot
            //   - `self_val` — the receiver. For inline-allocated
            //     receivers like `Ghost.new.poof`, the Object isn't
            //     bound to any caller local, so this window is the
            //     only thing keeping it alive
            //   - `block` (when Some) — heap-resident `BlockHandle`
            //     not yet attached to the new frame
            //   - `kw_hash` keys+values (when present) — the Hash
            //     contents were cloned out earlier; the per-pair
            //     Values may be heap-y and need to survive until
            //     the kw_count > 0 branch below reads them.
            //
            // Master commit 01b28ed shipped a narrower version of
            // this guard (pinning only `self_val` + `rest_vec`).
            // This widens it to `locals` / `block` / `kw_hash` and
            // adds the `check_alloc?` the original cut was missing
            // — a host configured with `max_heap_objects` would
            // otherwise see the rest-Array silently slip past the
            // cap, since `heap.alloc` itself doesn't enforce it.
            // The PinGuard's Drop pops on the early-return path of
            // `check_alloc?` too, so adding the check is safe.
            let rest_vec: Vec<Value> = args_iter.collect();
            let rest_slot = positional_max;
            let arr_id = {
                let mut g = PinGuard::new(self);
                for v in &locals { g.pin(v.clone()); }
                for v in &rest_vec { g.pin(v.clone()); }
                g.pin(self_val.clone());
                if let Some(id) = block { g.pin(Value::Block(id)); }
                if let Some(kw) = &kw_hash {
                    for (k, v) in kw {
                        g.pin(k.clone());
                        g.pin(v.clone());
                    }
                }
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                g.vm.heap.alloc(HeapObj::Array(rest_vec))
            };
            locals[rest_slot] = Value::Array(arr_id);
        }
        // Bind keyword params. kw names live at the tail of
        // m.params; for each, look up the corresponding key in
        // the kw_hash (Symbol-keyed). Missing required keyword
        // → ArgumentError. Missing optional → use literal default.
        let kw_start = positional_max + if has_rest { 1 } else { 0 };
        if kw_count > 0 {
            for (i, (default, kw_name)) in kw_defaults_snapshot.iter()
                .zip(m.params[kw_start..kw_start + kw_count].iter())
                .enumerate()
            {
                let key_sym = self.interner.intern(kw_name);
                let key_val = Value::Sym(key_sym);
                let found = kw_hash.as_ref().and_then(|h| {
                    h.iter().find(|(k, _)| k.ruby_eq(&key_val, &self.heap))
                        .map(|(_, v)| v.clone())
                });
                match (found, default) {
                    (Some(v), _) => locals[kw_start + i] = v,
                    (None, Some(d)) => locals[kw_start + i] = d.clone(),
                    (None, None) => return Err(self.trap(RubyError::ArgumentError {
                        msg: format!("missing keyword: :{}", kw_name),
                    })),
                }
            }
        }
        // **kw_rest binding. Take the kw_hash entries whose keys
        // weren't claimed by a named kw_param above and collect
        // them into a fresh Hash bound to the kw_rest slot. With
        // no kw_hash at all (caller passed no kwargs), the slot
        // still gets a fresh empty Hash so `**opts` reliably yields
        // a Hash to user code. The known-names set is built from
        // the same kw_param name slice we just zipped over.
        if has_kw_rest {
            let kw_rest_slot = kw_start + kw_count;
            let known_keys: Vec<Value> = m.params[kw_start..kw_start + kw_count]
                .iter()
                .map(|nm| Value::Sym(self.interner.intern(nm)))
                .collect();
            let leftover: Vec<(Value, Value)> = match &kw_hash {
                Some(h) => h.iter()
                    .filter(|(k, _)| !known_keys.iter().any(|kk| kk.ruby_eq(k, &self.heap)))
                    .cloned()
                    .collect(),
                None => Vec::new(),
            };
            // Same GC root-hole pattern as the rest-arg path above
            // (and the master Array#zip / Hash#sort_by chain fixed
            // in earlier PRs): `locals` / `self_val` / `block` /
            // `kw_hash` / `leftover` are Rust locals, NOT on
            // vm.stack / pinned, so the explicit `maybe_gc()` here
            // sweeps any heap-backed values they reference. Pin
            // everything participating in the new Hash alloc + the
            // already-bound locals through the alloc point.
            //
            // Master shipped the kw_rest code without this guard
            // (commits 680dbef "Module include chain + is_a?" /
            // ed0b872 "nested block destructure"); STRESS_GC tests
            // `anon_kwrest` and `kwrest_args` were the canary.
            let hid = {
                let mut g = PinGuard::new(self);
                for v in &locals { g.pin(v.clone()); }
                g.pin(self_val.clone());
                if let Some(id) = block { g.pin(Value::Block(id)); }
                if let Some(kw) = &kw_hash {
                    for (k, v) in kw {
                        g.pin(k.clone());
                        g.pin(v.clone());
                    }
                }
                for (k, v) in &leftover {
                    g.pin(k.clone());
                    g.pin(v.clone());
                }
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                g.vm.heap.alloc(HeapObj::Hash(leftover))
            };
            locals[kw_rest_slot] = Value::Hash(hid);
        }
        // `&blk` named block param: bind the caller's block (if any)
        // into the trailing block_param slot as `Value::Block(id)`,
        // or `Value::Nil` if no block was passed. The slot lives at
        // the very end of `params` after kw_rest (see Proto.block_param
        // / compile_proto for layout).
        if has_block_param {
            let block_slot = positional_max
                + if has_rest { 1 } else { 0 }
                + kw_count
                + if has_kw_rest { 1 } else { 0 };
            locals[block_slot] = match block {
                Some(id) => Value::Block(id),
                None => Value::Nil,
            };
        }
        self.frames.push(Frame {
            proto_idx: m.proto_idx,
            ip: 0,
            locals: Rc::new(RefCell::new(locals)),
            self_val,
            base_sp: self.stack.len(),
            is_class_body: false, swap_return: None, block_arg: block, defining_class: m.defining_class.as_ref().and_then(|w| w.upgrade()), is_block: false,
            // Drives the body's default-arg prologue. Slots
            // `[0, positional_take)` came from the caller; slots
            // `[positional_take, positional_max)` are left Nil
            // here and the prologue's `Op::JumpIfArgGiven` skips
            // the default-eval for the former, executes it for
            // the latter.
            n_given_positional: positional_take as u16,
            rescues: vec![], loop_rescue_depths: vec![], loop_stack_depths: vec![],
        });
        Ok(())
    }



    /// `obj.instance_eval { |o| ... }` / `cls.class_eval { |c| ... }`
    /// — invoke the block with `self` swapped to `new_self`.
    ///
    /// When `as_class_body` is true (the `class_eval` case),
    /// we also push `cls` onto `class_stack` + a fresh
    /// `Public` visibility entry, and mark the new frame
    /// `is_class_body: true`. That re-uses the existing
    /// class-body machinery so `def name; …; end` inside the
    /// block lands on the receiver class's method table — the
    /// dominant DSL use of `class_eval`. The cost: per the
    /// existing class-body Return semantics
    /// (`vm/step.rs::Op::Return`), the frame returns the class
    /// itself rather than the block's last expression. CRuby
    /// returns the block value; we'll need a non-`is_class_body`
    /// path to match exactly when a real use-case appears (see
    /// SUBSET.md). For `instance_eval` (`as_class_body=false`)
    /// the frame is a normal block, so the block's last
    /// expression is the return value — that part matches CRuby.
    ///
    /// `instance_eval { def name; ...; end }` defines a
    /// *singleton* method on the receiver in CRuby. rubyrs
    /// doesn't model singleton classes yet; `def` inside an
    /// `instance_eval` block lands on `toplevel_methods` (the
    /// same documented divergence as `attr_*` / `alias_method` /
    /// `define_method` outside a class body — see SUBSET.md's
    /// PoC caveat list). Real uses of `instance_eval` in our
    /// niche (configuration DSLs) typically read state rather
    /// than define methods, so this is acceptable for now.
    pub(crate) fn invoke_block_with_self(
        &mut self,
        block_id: ObjId,
        new_self: Value,
        as_class_body: bool,
        args: Vec<Value>,
    ) -> Result<(), Trap> {
        self.check_frames()?;
        let (proto_idx, captured, param_start, n_params) = {
            let bh = self.heap.block(block_id);
            (bh.proto_idx, bh.captured.clone(), bh.param_start, bh.n_params)
        };
        // Bind args into the block's param slots, same auto-splat
        // shape as `invoke_block`. For instance_eval/class_eval
        // the conventional arg is a single value (self), so the
        // single-Array auto-splat case is unlikely to trigger,
        // but we keep the rule identical to avoid surprising
        // future callers.
        let args: Vec<Value> = if n_params > 1 && args.len() == 1 {
            match &args[0] {
                Value::Array(aid) => self.heap.array(*aid).clone(),
                _ => args,
            }
        } else {
            args
        };
        let proto = &self.protos[proto_idx];
        let needed = proto.n_locals as usize;
        {
            let mut locals = captured.borrow_mut();
            if locals.len() < needed {
                while locals.len() < needed { locals.push(Value::Nil); }
            }
            for (i, a) in args.into_iter().enumerate() {
                if i < n_params as usize {
                    locals[param_start as usize + i] = a;
                }
            }
        }
        if as_class_body {
            // class_eval: re-use the class-body machinery so
            // `def` inside the block goes onto cls's method
            // table. Mirrors what `Op::DefClass` does at the
            // top of a `class X ... end` body. The return-path
            // handlers in vm/step.rs pop both stacks when this
            // frame returns, keyed off `is_class_body: true`.
            if let Value::Class(cls) = &new_self {
                self.class_stack.push(cls.clone());
                self.class_visibility_stack.push(crate::value::Visibility::Public);
            } else {
                // Caller checked Type before getting here, so
                // this is a programmer-error path. ICE rather
                // than silent-corruption: the class_stack pop
                // on frame return would underflow.
                panic!("ICE: invoke_block_with_self as_class_body=true requires Value::Class new_self");
            }
        }
        self.frames.push(Frame {
            proto_idx,
            ip: 0,
            locals: captured,
            self_val: new_self,
            base_sp: self.stack.len(),
            is_class_body: as_class_body,
            swap_return: None,
            block_arg: None,
            defining_class: None,
            // class_eval's frame is BOTH `is_block: true` and
            // `is_class_body: true`. That dual role matters for
            // non-local `return`: per the unwind loop in
            // `vm/step.rs` (Op::ReturnMethod's branch), a
            // `return` inside the block walks back through
            // is_block frames to find the enclosing method.
            // With `is_block: false` the class_eval frame would
            // be the target itself — `return` would return *from
            // class_eval* rather than the enclosing method,
            // diverging from CRuby. The matching unwind change
            // (pop class_stack/visibility_stack when walking
            // past a `is_block && is_class_body` frame) lives
            // in `vm/step.rs`.
            is_block: true,
            n_given_positional: 0,
            rescues: vec![], loop_rescue_depths: vec![], loop_stack_depths: vec![],
        });
        Ok(())
    }

    /// Wrap a BoundMethod into a fresh `Value::Block` so it can
    /// be passed wherever a block is expected. Lazily compiles a
    /// single shared forwarder proto on first call; subsequent
    /// calls reuse the same proto index. The synthesised
    /// BlockHandle stashes the BoundMethod in `captured[0]` and
    /// uses the proto's rest slot to splat the caller's args
    /// into a `.call(...)` on it.
    pub(crate) fn coerce_bound_method_to_block(&mut self, bm_id: crate::value::ObjId)
        -> Result<crate::value::ObjId, Trap>
    {
        use crate::bytecode::{Op, Proto};
        use crate::error::Span;
        use crate::heap::HeapObj;
        use std::cell::RefCell;

        // Lazy proto build. Locals layout:
        //   slot 0: the BoundMethod (captured)
        //   slot 1: args Array (rest slot, filled by invoke_block)
        let proto_idx = if let Some(idx) = self.bound_method_forwarder_proto {
            idx
        } else {
            let call_id = self.interner.intern("call");
            let proto = Proto {
                name: "<bound-method-forwarder>".to_string(),
                params: Vec::new(),
                n_required_positional: 0,
                rest_param: None,
                kw_param_defaults: Vec::new(),
                kw_rest_param: None,
                block_param: None,
                n_locals: 2,
                code: vec![
                    Op::LoadLocal(0),
                    Op::LoadLocal(1),
                    Op::ApplyCall(call_id, u16::MAX),
                    Op::Return,
                ],
                op_spans: vec![Span::ZERO; 4],
                filename: "<synthetic>".into(),
                // Synthetic forwarder protos have no body-
                // introduced locals; every slot they touch is
                // either filled at invoke time (block params /
                // rest) or written by the proto's own emitted
                // ops. `u16::MAX` skips the per-invocation reset.
                block_body_local_start: u16::MAX,
                byte_literals: Vec::new(),
            };
            let idx = self.protos.len();
            self.protos.push(proto);
            self.bound_method_forwarder_proto = Some(idx);
            idx
        };

        // captured[0] = the BoundMethod; captured[1] left to
        // invoke_block to populate with the rest Array.
        //
        // Pin the BoundMethod across maybe_gc — the Rc<RefCell<Vec>>
        // we just built is a Rust-local with no GC root yet (the
        // Block that would own it isn't alloc'd until after the
        // maybe_gc). Without the pin, STRESS_GC sweeps the
        // BoundMethod slot between Vec construction and Block alloc;
        // the new Block alloc reuses the freed slot, and the
        // captured BoundMethod ObjId silently points at the Block
        // itself — invoke_block then panics with "BoundMethod slot
        // holds non-BoundMethod" when `.call` dispatches.
        let captured = Rc::new(RefCell::new(vec![Value::BoundMethod(bm_id), Value::Nil]));
        let mut g = crate::vm::PinGuard::new(self);
        g.pin(Value::BoundMethod(bm_id));
        g.vm.maybe_gc();
        g.vm.check_alloc()?;
        let id = g.vm.heap.alloc(HeapObj::Block(crate::value::BlockHandle {
            proto_idx,
            captured,
            self_val: Value::Nil,
            param_start: 0,
            n_params: 0,
            rest_slot: Some(1),
        }));
        Ok(id)
    }

    /// Build a `Value::Block` that, when called with `*args`,
    /// invokes `outer.call(inner.(*args))`. Used by Method#`>>` /
    /// `<<` to express function composition. Both sides must be
    /// callable (BoundMethod or Block); validated by the caller.
    /// Proto is lazy-built and shared across all composition sites.
    pub(crate) fn coerce_compose_to_block(
        &mut self,
        outer: Value,
        inner: Value,
    ) -> Result<crate::value::ObjId, Trap> {
        use crate::bytecode::{Op, Proto};
        use crate::error::Span;
        use crate::heap::HeapObj;
        use std::cell::RefCell;

        // Locals layout:
        //   slot 0: outer callable (runs second)
        //   slot 1: inner callable (runs first)
        //   slot 2: args Array (filled via rest_slot)
        let proto_idx = if let Some(idx) = self.method_compose_forwarder_proto {
            idx
        } else {
            let call_id = self.interner.intern("call");
            let proto = Proto {
                name: "<method-compose-forwarder>".to_string(),
                params: Vec::new(),
                n_required_positional: 0,
                rest_param: None,
                kw_param_defaults: Vec::new(),
                kw_rest_param: None,
                block_param: None,
                n_locals: 3,
                code: vec![
                    Op::LoadLocal(0),                   // [outer]
                    Op::LoadLocal(1),                   // [outer, inner]
                    Op::LoadLocal(2),                   // [outer, inner, args]
                    Op::ApplyCall(call_id, u16::MAX),   // [outer, inner_result]
                    Op::Call(call_id, 1, u16::MAX),     // [outer_result]
                    Op::Return,
                ],
                op_spans: vec![Span::ZERO; 6],
                filename: "<synthetic>".into(),
                // Synthetic forwarder protos have no body-
                // introduced locals; every slot they touch is
                // either filled at invoke time (block params /
                // rest) or written by the proto's own emitted
                // ops. `u16::MAX` skips the per-invocation reset.
                block_body_local_start: u16::MAX,
                byte_literals: Vec::new(),
            };
            let idx = self.protos.len();
            self.protos.push(proto);
            self.method_compose_forwarder_proto = Some(idx);
            idx
        };
        let captured = Rc::new(RefCell::new(vec![outer.clone(), inner.clone(), Value::Nil]));
        let mut g = crate::vm::PinGuard::new(self);
        g.pin(outer);
        g.pin(inner);
        g.vm.maybe_gc();
        g.vm.check_alloc()?;
        let id = g.vm.heap.alloc(HeapObj::Block(crate::value::BlockHandle {
            proto_idx,
            captured,
            self_val: Value::Nil,
            param_start: 0,
            n_params: 0,
            rest_slot: Some(2),
        }));
        Ok(id)
    }

    pub(crate) fn invoke_block(&mut self, block_id: ObjId, args: Vec<Value>) -> Result<(), Trap> {
        self.check_frames()?;
        // Snapshot what we need out of the block's heap slot before
        // taking any `&mut self` action. BlockHandle.captured is a
        // shared `Rc<RefCell<Vec<Value>>>` — cheap to clone.
        let (proto_idx, captured, self_val, param_start, n_params, rest_slot) = {
            let bh = self.heap.block(block_id);
            (bh.proto_idx, bh.captured.clone(), bh.self_val.clone(),
             bh.param_start, bh.n_params, bh.rest_slot)
        };
        // CRuby auto-splat: when a block declared with >1 parameter
        // is called with a single Array argument, the Array's
        // elements are spread into the parameter slots. The most
        // common ergonomic surfaces this enables:
        //   arr_of_pairs.each { |a, b| ... }       # arr = [[1,2], [3,4]]
        //   hash.each_with_index { |(k, v), i| }   # pair + index
        //   hash.to_a.sort_by { |k, v| v }         # pair after Hash#to_a
        // Hash#each / #map already yield two args directly, so this
        // path doesn't change their behaviour. Single-param blocks
        // also unaffected — they bind the whole Array.
        //
        // Auto-splat doesn't apply to rest-param blocks — `|*args|`
        // wants to capture the whole arg list, including a single
        // Array as-is.
        let args: Vec<Value> = if n_params > 1 && args.len() == 1 && rest_slot.is_none() {
            match &args[0] {
                Value::Array(aid) => self.heap.array(*aid).clone(),
                _ => args,
            }
        } else {
            args
        };
        // Build the rest Array (if any) BEFORE taking the locals
        // borrow — heap.alloc needs &mut self.heap, which conflicts
        // with the captured.borrow_mut() below.
        //
        // GC rooting: at this point the caller has popped the
        // Value::Block(block_id) off the operand stack (see
        // `do_call_block` and the Block.call arm in `do_call`),
        // so the only live reference to the block + its captured
        // Vec is this fn's `block_id` parameter — *not* a GC root.
        // Without pinning, the maybe_gc below would sweep the
        // Block's heap slot and (transitively) every captured
        // BoundMethod/Block held inside `captured`. The new alloc
        // could reuse the freed slot, and the forwarder would
        // dispatch through a dangling ObjId. Reproduced under
        // STRESS_GC=1 by `proc_curry_compose.rb`'s `(succ >> m).(4)`
        // — composing a Block with a BoundMethod produces a
        // compose-forwarder Block with `rest_slot = Some(2)`, so
        // this branch fires; the Squared instance held inside
        // `m`'s BoundMethod gets swept between pop and the
        // recursive `m.call`, panicking later at heap.rs's
        // `class_of called on non-Object slot`.
        let rest_array_val = if let Some(slot) = rest_slot {
            let rest_args: Vec<Value> = args.iter().skip(n_params as usize).cloned().collect();
            // Truncate args to the leading required slots — the
            // overflow now lives in rest_args.
            let mut g = crate::vm::PinGuard::new(self);
            g.pin(Value::Block(block_id));
            g.vm.maybe_gc();
            g.vm.check_alloc()?;
            let id = g.vm.heap.alloc(HeapObj::Array(rest_args));
            Some((slot, Value::Array(id)))
        } else {
            None
        };
        let proto = &self.protos[proto_idx];
        let needed = proto.n_locals as usize;
        let body_local_start = proto.block_body_local_start;
        {
            let mut locals = captured.borrow_mut();
            if locals.len() < needed {
                while locals.len() < needed { locals.push(Value::Nil); }
            }
            // Reset body-introduced block-local slots before
            // rebinding params. CRuby's "block-locals are fresh
            // each invocation" semantics: a variable
            // first-assigned inside the block body (e.g.
            // `y = 100 if cond`, `n ||= 0`, plain `tmp = expr`)
            // sees `nil` at the top of every call, even when an
            // earlier invocation assigned it. Outer-scope
            // variables (slot index < parent.n_locals at compile
            // time) and the block's own params keep their
            // values across invocations because their slot
            // indices sit below `body_local_start`.
            //
            // `block_body_local_start == u16::MAX` is the
            // sentinel for "not a block-shaped proto" — set by
            // `ProtoBuilder::build` and by the cext synthetic
            // forwarders. The branch is also a no-op when the
            // block body assigned no new locals (start equals
            // n_locals).
            if (body_local_start as usize) < needed {
                for slot in body_local_start as usize..needed {
                    locals[slot] = Value::Nil;
                }
            }
            // Place args into the block's required param slots.
            // CRuby's arity-mismatch semantics: too few args →
            // leftover slots bind to Nil. Overflow past n_params
            // either flows into the rest slot (handled below) or
            // is silently dropped (block-arity-permissive default).
            let mut it = args.into_iter();
            for i in 0..n_params as usize {
                locals[param_start as usize + i] = it.next().unwrap_or(Value::Nil);
            }
            if let Some((slot, val)) = rest_array_val {
                locals[slot as usize] = val;
            }
        }
        self.frames.push(Frame {
            proto_idx,
            ip: 0,
            locals: captured,
            self_val,
            base_sp: self.stack.len(),
            is_class_body: false, swap_return: None, block_arg: None, defining_class: None,
            is_block: true, n_given_positional: 0, rescues: vec![], loop_rescue_depths: vec![], loop_stack_depths: vec![],
        });
        Ok(())
    }



    pub(crate) fn do_call_block(&mut self, name_id: SymId, argc: usize, no_recv: bool, cache_id: u16) -> Result<(), Trap> {
        let name = self.interner.resolve(name_id).clone();
        // Consume `bypass_visibility_once` at the dispatch boundary
        // — same reasoning as `do_call`. `do_call_block` doesn't
        // have a visibility-check site of its own today (block-form
        // private/protected enforcement is a pre-existing gap), so
        // the consumed value is unused locally; the important
        // effect is that the flag can't leak past the block-form
        // `send`/`__send__` re-aim into the next unrelated call.
        let _bypass_visibility = std::mem::replace(&mut self.bypass_visibility_once, false);
        let split = self.stack.len() - argc;
        let args: Vec<Value> = self.stack.drain(split..).collect();
        let block_val = self.stack.pop().expect("ICE: stack underflow before block");
        let block = match block_val {
            Value::Block(id) => id,
            // `&method_object` forwarding (K8): coerce the
            // BoundMethod into a Block via `to_proc` semantics.
            // Synthesises a vararg-lambda whose captured locals
            // hold the BoundMethod; when invoked, it does
            // `m.call(*args)`. See `coerce_bound_method_to_block`.
            Value::BoundMethod(bm_id) => self.coerce_bound_method_to_block(bm_id)?,
            _ => panic!("ICE: CallBlock without Block value on stack"),
        };
        let recv = if no_recv {
            None
        } else {
            Some(self.stack.pop().expect("ICE: stack underflow before block receiver"))
        };

        // P2-13: `block` (now an ObjId in a Rust local) is no
        // longer rooted after popping off the stack. Each native
        // iterator driver (`iter_array_filter`, the inline
        // `each` / `map` arms, etc.) pins the block alongside its
        // source receiver, so we don't need a guard at the
        // dispatch boundary itself. The `invoke_method_with_block`
        // path on the no_recv / Object-recv branches doesn't
        // trigger GC before installing the block as the frame's
        // `block_arg`, so the gap is safe there too.
        //
        // `instance_eval` / `class_eval` / `module_eval` — swap
        // `self` for the duration of the block. Intercepted here
        // so the receiver-type dispatch below can't claim them
        // first (e.g. a future `Object#instance_eval` primitive
        // would shadow this). `args.is_empty()` keeps us out of
        // the way of any hypothetical user-defined
        // `instance_eval(arg)` that someone might define.
        if let Some(r) = &recv {
            let is_instance_eval = &*name == "instance_eval";
            let is_class_eval = &*name == "class_eval" || &*name == "module_eval";
            if (is_instance_eval || is_class_eval) && args.is_empty() {
                if is_class_eval && !matches!(r, Value::Class(_)) {
                    // Align with the existing wording for `include`
                    // (vm/dispatch.rs:171, :369) so error messages
                    // are consistent across the Module-receiver
                    // family.
                    return Err(self.trap(RubyError::TypeError {
                        msg: format!(
                            "wrong argument type {} (expected Module)",
                            r.type_name(),
                        ),
                    }));
                }
                // CRuby passes `self` as the sole block arg (so
                // `obj.instance_eval { |o| o == obj }` works);
                // mirror that. The single-arg matches the
                // common DSL shape `cls.class_eval { |k| ... }`.
                let block_args = vec![r.clone()];
                self.invoke_block_with_self(block, r.clone(), is_class_eval, block_args)?;
                return Ok(());
            }
        }
        if let Some(r) = &recv
            && let Some(v) = self.collection_call_block(r, &name, &args, block)? {
                self.stack.push(v);
                return Ok(());
            }

        if no_recv {
            // `lambda { ... }` / `proc { ... }` / `Proc.new { ... }`-
            // style block-to-Value capture. rubyrs doesn't
            // distinguish Lambda from Proc at runtime (the strict-
            // arity check is the documented gap in SUBSET.md), so
            // both names just hand the attached block back as a
            // Value::Block. `args.is_empty()` keeps us out of the
            // way of user-defined `lambda(arg)` shapes if anyone
            // overrides the name.
            if args.is_empty() && (&*name == "lambda" || &*name == "proc") {
                self.stack.push(Value::Block(block));
                return Ok(());
            }
            if let Some(res) = self.builtin_call(&name, &args) {
                let v = res?;
                // See suppress_call_result_push doc on Vm —
                // mirrors the no_recv path above.
                if self.suppress_call_result_push {
                    self.suppress_call_result_push = false;
                } else {
                    self.stack.push(v);
                }
                return Ok(());
            }
            if let Some(host) = self.host_fns.get(&name_id).cloned() {
                let v = self.invoke_host_fn(host, &args)?;
                self.stack.push(v);
                return Ok(());
            }
            // Bare `send(:foo) { ... }` / `__send__(:foo) { ... }`
            // — same re-aim as the no_recv arm in `do_call`. See
            // there for the override + visibility rationale.
            if matches!(&*name, "send" | "__send__") {
                let frame_self = self.frames.last()
                    .expect("ICE: do_call_block(no_recv) with empty frames")
                    .self_val.clone();
                let user_override = &*name == "send" && match &frame_self {
                    Value::Object(id) => {
                        let cls = self.heap.class_of(*id);
                        self.lookup_method_cached(&cls, name_id, cache_id).is_some()
                    }
                    Value::Class(c) => self.lookup_class_singleton_method(c, name_id).is_some(),
                    _ => false,
                };
                if !user_override {
                    let target_sym = self.parse_send_target(&args)?;
                    let new_argc = args.len() - 1;
                    self.bypass_visibility_once = true;
                    self.stack.push(Value::Block(block));
                    for a in args.into_iter().skip(1) {
                        self.stack.push(a);
                    }
                    return self.do_call_block(target_sym, new_argc, true, u16::MAX);
                }
            }
            let self_val = self.frames.last().expect("ICE: do_call_block no frame").self_val.clone();
            if let Value::Object(id) = &self_val {
                let cls = self.heap.class_of(*id);
                if let Some(m) = self.lookup_method_cached(&cls, name_id, cache_id) {
                    self.invoke_method_with_block(m, self_val.clone(), args, Some(block))?;
                    return Ok(());
                }
            }
            if let Some(m) = self.toplevel_methods.get(&name_id).cloned() {
                self.invoke_method_with_block(m, self_val, args, Some(block))?;
                return Ok(());
            }
            if self.try_method_missing(&self_val, name_id, args, Some(block))? {
                return Ok(());
            }
            return Err(self.trap(RubyError::NoMethodError {
                method: name.to_string(), recv_type: self_val.type_name(),
            }));
        }
        let recv = recv.expect("ICE: receiver missing for block call");

        // `obj.send(:name, args...) { ... }` — same dynamic-name
        // re-aim as the block-less arm in `do_call`. `do_call_block`
        // pops args then block then recv (in that drain/pop order),
        // so the stack shape it expects from a caller is
        // `[..., recv, block, *args]`. Put them back in that order
        // and re-enter. cache_id = u16::MAX for the same reason as
        // the block-less arm. User-`def send` override + visibility
        // bypass parity — same rules as the block-less arm; see
        // there for the rationale.
        let user_send_override = &*name == "send" && match &recv {
            Value::Object(id) => {
                let cls = self.heap.class_of(*id);
                self.lookup_method_cached(&cls, name_id, cache_id).is_some()
            }
            // `def self.send` on a class — singleton-method lookup
            // walking the class's superclass chain. Falls through to
            // the existing `Value::Class` arm which invokes the
            // user's singleton `send`.
            Value::Class(c) => self.lookup_class_singleton_method(c, name_id).is_some(),
            _ => false,
        };
        if matches!(&*name, "send" | "__send__") && !user_send_override {
            let target_sym = self.parse_send_target(&args)?;
            let new_argc = args.len() - 1;
            self.bypass_visibility_once = true;
            self.stack.push(recv);
            self.stack.push(Value::Block(block));
            for a in args.into_iter().skip(1) {
                self.stack.push(a);
            }
            return self.do_call_block(target_sym, new_argc, false, u16::MAX);
        }

        if let Some(v) = primitive_call(&recv, &name, &args, self.max_value_bytes).map_err(|e| self.trap(e))? { self.stack.push(v); return Ok(()); }
        if let Some(v) = self.sym_primitive(&recv, &name, &args) { self.stack.push(v); return Ok(()); }
        let new_id = self.interner.intern("new");
        if name_id == new_id
            && let Value::Class(cls) = &recv {
                // Pin args during the alloc window — see the matching
                // comment in `do_call`'s new-branch for the rationale.
                let id = {
                    let mut g = PinGuard::new(self);
                    for a in &args { g.pin(a.clone()); }
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    g.vm.heap.alloc(HeapObj::Instance(Instance {
                        class: cls.clone(), ivars: HashMap::new(),
                        singleton_class: None,
                    }))
                };
                let obj = Value::Object(id);
                let init_id = self.interner.intern("initialize");
                if let Some(m) = self.lookup_method_uncached(cls, init_id) {
                    self.invoke_method_with_block(m, obj.clone(), args, Some(block))?;
                    self.frames.last_mut().expect("ICE: frames empty after new").swap_return = Some(obj);
                } else {
                    self.stack.push(obj);
                }
                return Ok(());
            }
        if let Value::Object(id) = &recv {
            let cls = self.heap.class_of(*id);
            if let Some(m) = self.lookup_method_cached(&cls, name_id, cache_id) {
                self.invoke_method_with_block(m, recv.clone(), args, Some(block))?;
                return Ok(());
            }
        }
        if self.try_method_missing(&recv, name_id, args, Some(block))? {
            return Ok(());
        }
        Err(self.trap(RubyError::NoMethodError {
            method: name.to_string(), recv_type: recv.type_name(),
        }))
    }


}

/// Identity comparison for Method receivers — heap-managed
/// recvs compare by ObjId / Rc-pointer; primitives compare by
/// value. Matches CRuby's `equal?`-style semantics, narrowed to
/// the cases that can appear in a BoundMethod recv slot.
/// True for class names whose instances are backed by a non-Object
/// `Value` variant. Used by `Class#instance_method` to decide
/// between "real lookup, NameError on miss" (user class — the
/// methods table is the source of truth) and "synthesise an
/// UnboundMethod, let downstream arity / parameters fall back to
/// the builtin sentinel" (primitive — methods live in
/// `primitive_call` arms, not in a per-class table).
///
/// Mirrors `Vm::class_of`'s class-name set (`vm/lookup.rs`),
/// PLUS one intentional extra: `Kernel`. Kernel's instances are
/// Object-backed in CRuby (not a distinct `Value` variant), but
/// we treat it as a sentinel primitive here so
/// `Kernel.instance_method(:foo)` resolves via the same path as
/// real primitives — see the Kernel arm below for the rationale.
/// The two lists are NOT meant to be kept identical; future
/// editors should add new entries to whichever side actually
/// needs them.
/// `Class#method_defined?(name)` resolver. Walks the user-Method
/// table + ancestor chain first; if that misses and `cls` is a
/// primitive class (Integer / String / ...), builds a sentinel
/// receiver of the matching `Value` shape and consults the per-
/// primitive `responds_to` whitelist. This way
/// `String.method_defined?(:nope)` correctly returns `false` while
/// `Symbol.method_defined?(:name)` returns `true` (the
/// `msgpack-ruby/lib/msgpack/symbol.rb` Ruby-2.7+ version-detect
/// path). Excluded primitives that need a non-trivial sentinel
/// (Array/Hash/Range/Regexp/Proc/Method/UnboundMethod) fall back
/// to a permissive `true` — matches CRuby for the broadly-shared
/// Kernel methods and stays out of false-negative territory while
/// the synthesis cost isn't justified.
fn class_method_defined(vm: &mut Vm, cls: &Rc<Class>, sid: SymId) -> bool {
    if vm.lookup_method_uncached(cls, sid).is_some() {
        return true;
    }
    let sentinel: Option<Value> = match cls.name.as_str() {
        "Integer" => Some(Value::Int(0)),
        "Float" => Some(Value::Float(0.0)),
        "String" => Some(Value::new_str("")),
        // Sym(SymId(0)) is the first interned token — the
        // interner always has at least one entry by the time
        // class objects exist, so this is safe to construct.
        "Symbol" => Some(Value::Sym(SymId(0))),
        "TrueClass" => Some(Value::Bool(true)),
        "FalseClass" => Some(Value::Bool(false)),
        "NilClass" => Some(Value::Nil),
        _ => None,
    };
    match sentinel {
        Some(s) => vm.responds_to(&s, sid),
        // Aggregate / opaque primitives: keep the previously-
        // permissive answer so the gem helper path doesn't trip
        // on Kernel-shared method probes.
        None => is_primitive_class_name(&cls.name),
    }
}

impl Vm {
    /// Does an instance of the primitive class `class_name`
    /// respond to method `sid`? Builds a sentinel `Value` of
    /// the matching shape and consults the per-primitive
    /// `responds_to` whitelist. Aggregate primitives
    /// (Array/Hash/Range/Regexp/...) fall back to permissive
    /// `true`, matching `class_method_defined`'s shape. Used
    /// by `Op::AliasMethod` to decide whether to synthesise a
    /// primitive-forwarder Method when the source name isn't
    /// in the user-Method table.
    pub(crate) fn primitive_class_responds_to(&self, class_name: &str, sid: SymId) -> bool {
        let sentinel: Option<Value> = match class_name {
            "Integer" => Some(Value::Int(0)),
            "Float" => Some(Value::Float(0.0)),
            "String" => Some(Value::new_str("")),
            "Symbol" => Some(Value::Sym(SymId(0))),
            "TrueClass" => Some(Value::Bool(true)),
            "FalseClass" => Some(Value::Bool(false)),
            "NilClass" => Some(Value::Nil),
            _ => None,
        };
        match sentinel {
            Some(s) => self.responds_to(&s, sid),
            None => is_primitive_class_name(class_name),
        }
    }

    /// Build a Method that forwards to a primitive method on
    /// `self`. Emitted as the body of an `alias_method`'d
    /// primitive — when the alias is invoked, the body runs
    /// `LoadSelf; LoadLocal(0); ApplyCall(orig_id, ...); Return`
    /// so any args the caller passed flow through to the
    /// primitive call via the rest-Array slot. The forwarder
    /// Proto is appended to `self.protos` and the index is
    /// stamped into the returned Method.
    pub(crate) fn synth_primitive_forwarder(&mut self, cls: &Rc<Class>, orig_id: SymId) -> Rc<crate::value::Method> {
        use crate::bytecode::{Op, Proto};
        use crate::error::Span;
        let proto = Proto {
            name: format!("<primitive-alias-forwarder:{}>", self.interner.resolve(orig_id)),
            // `args` is the rest-arg name; proto.params lists it so
            // `invoke_method`'s arg-binding loop treats slot 0 as
            // the rest collector. n_required_positional = 0 keeps
            // the alias arity-permissive (matches primitive
            // dispatch, which is variadic).
            params: vec!["args".to_string()],
            n_required_positional: 0,
            rest_param: Some("args".to_string()),
            kw_param_defaults: vec![],
            kw_rest_param: None,
            block_param: None,
            n_locals: 1,
            code: vec![
                Op::LoadSelf,
                Op::LoadLocal(0),
                Op::ApplyCall(orig_id, u16::MAX),
                Op::Return,
            ],
            op_spans: vec![Span::ZERO; 4],
            filename: "<primitive-alias>".into(),
            block_body_local_start: u16::MAX,
            byte_literals: vec![],
        };
        let idx = self.protos.len();
        self.protos.push(proto);
        Rc::new(crate::value::Method {
            params: vec!["args".to_string()],
            proto_idx: idx,
            defining_class: Some(Rc::downgrade(cls)),
            visibility: std::cell::Cell::new(crate::value::Visibility::Public),
            closure: None,
        })
    }
}

fn is_primitive_class_name(name: &str) -> bool {
    matches!(
        name,
        "Integer" | "Float" | "String" | "Symbol"
            | "Array" | "Hash" | "Range"
            | "Regexp" | "Proc"
            | "Method" | "UnboundMethod"
            | "TrueClass" | "FalseClass" | "NilClass"
            // Kernel — modeled as a sentinel "primitive" so
            // `Kernel.instance_method(:foo)` resolves without
            // forcing every Kernel method to live in a class
            // table. Real CRuby: Kernel is a Module included in
            // Object, transitively giving every value its method
            // set. We don't have Modules; this sentinel makes
            // the lookup succeed and emits an UnboundMethod that
            // defers resolution to bind+call (where do_call
            // routes to the receiver's primitive method dispatch
            // as if the call were direct).
            | "Kernel"
    )
}

fn method_recv_identity(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => x == y,
        (Value::Array(x), Value::Array(y)) => x == y,
        (Value::Hash(x), Value::Hash(y)) => x == y,
        (Value::Range(x), Value::Range(y)) => x == y,
        (Value::Block(x), Value::Block(y)) => x == y,
        (Value::BoundMethod(x), Value::BoundMethod(y)) => x == y,
        (Value::UnboundMethod(x), Value::UnboundMethod(y)) => x == y,
        // ObjId identity, matching `method_recv_hash`. Two BigInt
        // Values are "the same receiver" only when they share an
        // ObjId; canonical-value equality (e.g. comparing two
        // independently-allocated 2^64 BigInts) is intentionally
        // not the relation here — `bound_method == other` only
        // collapses when the underlying receiver is literally the
        // same heap slot.
        #[cfg(feature = "bignum")]
        (Value::BigInt(x), Value::BigInt(y)) => x == y,
        (Value::Class(x), Value::Class(y)) => Rc::ptr_eq(x, y),
        (Value::Str(x), Value::Str(y)) => Rc::ptr_eq(x, y),
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => x == y,
        (Value::Sym(x), Value::Sym(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Nil, Value::Nil) => true,
        _ => false,
    }
}

/// Hash a Method receiver consistently with `method_recv_identity`.
/// Two receivers that compare equal via `method_recv_identity`
/// must collide here.
fn method_recv_hash(v: &Value) -> i64 {
    match v {
        Value::Object(id) | Value::Array(id) | Value::Hash(id) | Value::Range(id)
        | Value::Block(id) | Value::BoundMethod(id) | Value::UnboundMethod(id)
        | Value::CurriedProc(id) => id.0 as i64,
        // Two BigInts that hash-equal must collide via ObjId since
        // the heap-side bigint value identity is the ObjId (we
        // never share an ObjId across different BigInt values).
        #[cfg(feature = "bignum")]
        Value::BigInt(id) => id.0 as i64,
        Value::Class(c) => Rc::as_ptr(c) as i64,
        Value::Str(s) => Rc::as_ptr(s) as i64,
        Value::Int(n) => *n,
        Value::Float(f) => f.to_bits() as i64,
        Value::Sym(s) => s.0 as i64,
        Value::Bool(true) => 1,
        Value::Bool(false) => 0,
        Value::Nil => 0xDEAD_BEEF,
        #[cfg(feature = "regex")]
        Value::Regex(r) => Rc::as_ptr(r) as i64,
    }
}
