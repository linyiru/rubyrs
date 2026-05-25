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

#[cfg(not(target_os = "wasi"))]
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
    #[cfg(not(target_os = "wasi"))]
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
    /// re-entrant `rb_funcall` can find the running VM (ADR 0013).
    /// V1 closures hold no Rust borrow of `self` during the call, so
    /// the raw-ptr reborrow inside cext is the only access path and
    /// aliasing is well-defined.
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
                #[cfg(not(target_os = "wasi"))]
                {
                    let vm_ptr: *mut Vm = self;
                    with_vm_ptr_set(vm_ptr, || host(args))
                }
                #[cfg(target_os = "wasi")]
                { host(args) }
            }
            HostFnSlot::V2(host) => {
                let ctx = HostCtx::new(&self.heap, &self.interner);
                host(&ctx, args)
            }
        }
    }

    pub(crate) fn do_call(&mut self, name_id: SymId, argc: usize, no_recv: bool, cache_id: u16) -> Result<(), Trap> {
        let name = self.interner.resolve(name_id).clone();
        let split = self.stack.len() - argc;
        let args: Vec<Value> = self.stack.drain(split..).collect();
        let recv = if no_recv {
            None
        } else {
            Some(self.stack.pop().expect("ICE: stack underflow before do_call receiver"))
        };

        if no_recv {
            if let Some(res) = self.builtin_call(&name, &args) {
                self.stack.push(res?);
                return Ok(());
            }
            if let Some(host) = self.host_fns.get(&name_id).cloned() {
                let v = self.invoke_host_fn(host, &args)?;
                self.stack.push(v);
                return Ok(());
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
                    self.maybe_gc();
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
                                Value::Str(s) => Some(self.interner.intern(&s.borrow())),
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

        if let Some(v) = primitive_call(&recv, &name, &args, self.max_value_bytes)
            .map_err(|e| self.trap(e))? {
            self.stack.push(v);
            return Ok(());
        }
        if let Some(v) = self.sym_primitive(&recv, &name, &args) {
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
                let obj = if let Some(alloc_func) = cls.cext_alloc_func.get() {
                    #[cfg(not(target_os = "wasi"))]
                    {
                        let class_name = cls.name.clone();
                        // arity=0 (self-only) is the alloc_func ABI:
                        // VALUE allocate(VALUE klass). cext_dispatch
                        // builds [self_handle] internally and
                        // invoke_with_raise's case 0 calls func(self).
                        // CURRENT_VM_PTR must be set before the cext
                        // can rb_funcall back into the Vm (and so
                        // rb_data_typed_object_wrap can find the Vm
                        // to allocate on its heap).
                        let qualified = format!("{}::allocate", class_name);
                        let vm_ptr: *mut Vm = self;
                        super::cext::with_vm_ptr_set(vm_ptr, || {
                            super::cext::cext_dispatch(
                                &qualified,
                                alloc_func,
                                0,
                                &[],
                                super::cext::CextSelfHandle::Class(&class_name),
                            )
                        })?
                    }
                    #[cfg(target_os = "wasi")]
                    {
                        // wasi: cext path is stubbed; fall back to
                        // plain Instance allocation.
                        let mut g = PinGuard::new(self);
                        for a in &args { g.pin(a.clone()); }
                        g.vm.maybe_gc();
                        g.vm.check_alloc()?;
                        let id = g.vm.heap.alloc(HeapObj::Instance(Instance {
                            class: cls.clone(),
                            ivars: HashMap::new(),
                            singleton_class: None,
                        }));
                        Value::Object(id)
                    }
                } else {
                    // `args` and `recv` were popped off the operand
                    // stack by do_call's setup; while we're about to
                    // trigger GC via `maybe_gc`, they exist only as
                    // Rust locals. Pin any heap values inside `args`
                    // (Class is `Rc`-managed and doesn't need pinning)
                    // so the GC's root walk sees them. The
                    // `check_alloc()?` inside the guard is now safe —
                    // the guard's Drop pops on the early-return path.
                    let id = {
                        let mut g = PinGuard::new(self);
                        for a in &args { g.pin(a.clone()); }
                        g.vm.maybe_gc();
                        g.vm.check_alloc()?;
                        g.vm.heap.alloc(HeapObj::Instance(Instance {
                            class: cls.clone(),
                            ivars: HashMap::new(),
                            singleton_class: None,
                        }))
                    };
                    Value::Object(id)
                };
                let init_id = self.interner.intern("initialize");
                if let Some(m) = self.lookup_method_uncached(cls, init_id) {
                    // Ruby-defined initialize takes precedence.
                    self.invoke_method(m, obj.clone(), args)?;
                    self.frames.last_mut().expect("ICE: frames empty after new").swap_return = Some(obj);
                } else {
                    // L3-F: cext-defined initialize (registered via
                    // rb_define_method) lives in cext_instance_methods.
                    // Dispatch through the existing instance-method
                    // path if present — this picks up arity validation
                    // and rb_raise handling for free. Skip on
                    // arity-mismatch (variadic / -1 isn't supported by
                    // the setjmp shim yet) so allocation still
                    // succeeds; common case where Packer.new / Parser
                    // .new is called with no args still works because
                    // the cext-side state was already zero-init'd in
                    // the alloc_func.
                    #[cfg(not(target_os = "wasi"))]
                    {
                        let cext_init_reg = self.cext_instance_methods
                            .get(&cls.name)
                            .and_then(|t| t.get(&init_id).cloned())
                            .filter(|reg| (0..=5).contains(&reg.arity) && reg.arity as usize == args.len());
                        if let Some(reg) = cext_init_reg {
                            let qualified = reg.qualified_name.clone();
                            let func = reg.func;
                            let arity = reg.arity;
                            let obj_clone = obj.clone();
                            let args_ref = args.clone();
                            let vm_ptr: *mut Vm = self;
                            super::cext::with_vm_ptr_set(vm_ptr, || {
                                super::cext::cext_dispatch(
                                    &qualified, func, arity, &args_ref,
                                    super::cext::CextSelfHandle::Object(obj_clone),
                                )
                            })?;
                        }
                    }
                    self.stack.push(obj);
                }
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
                if vis == Visibility::Private {
                    return Err(self.trap(RubyError::NoMethodError {
                        method: format!("private method '{name}' called"),
                        recv_type: recv.type_name(),
                    }));
                }
                if vis == Visibility::Protected {
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
            #[cfg(not(target_os = "wasi"))]
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
            // User-Ruby `def self.foo` singletons: check the per-class
            // table populated by `Op::DefSingletonMethod`. Walks the
            // superclass chain — CRuby's metaclass model has the
            // singleton class of `Dog < Animal` inherit from the
            // singleton class of `Animal`, so `Dog.kingdom` finds
            // `Animal`'s `def self.kingdom`. We approximate the same
            // shape with a straight superclass walk over the
            // `singleton_methods` tables.
            let mut current = cls.clone();
            let user_singleton = loop {
                if let Some(m) = current.singleton_methods.borrow().get(&name_id).cloned() {
                    break Some(m);
                }
                let parent = current.superclass.borrow().clone();
                match parent {
                    Some(p) => current = p,
                    None => break None,
                }
            };
            if let Some(m) = user_singleton {
                let target_self = recv.clone();
                return self.invoke_method(m, target_self, args);
            }
            if &*cls.name == "File"
                && let Some(v) = self.file_class_dispatch(&name, &args)? {
                    self.stack.push(v);
                    return Ok(());
                }
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
        if &*name == "method" && args.len() == 1
            && let Value::Sym(bound_name_id) = &args[0] {
                self.maybe_gc();
                self.check_alloc()?;
                let id = self.heap.alloc(HeapObj::BoundMethod {
                    recv: recv.clone(),
                    name_id: *bound_name_id,
                });
                self.stack.push(Value::BoundMethod(id));
                return Ok(());
            }
        // `bm.call(args)` / `bm.()` / `bm[args]` — dispatch the
        // captured method on the captured receiver. We re-enter
        // `do_call` recursively with the bound recv pushed below
        // the args, the captured name interned, and the original
        // argc.
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
                _ => {}
            }
        }
        if let Some(v) = self.collection_call(&recv, &name, &args)? {
            self.stack.push(v);
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
                Value::Regex(re) => match arg {
                    Value::Str(s) => re.is_match(&s.borrow()),
                    _ => false,
                },
                _ => recv.ruby_eq(arg, &self.heap),
            };
            self.stack.push(Value::Bool(result));
            return Ok(());
        }
        // `=~` — Regex/String matching. Returns the byte offset of
        // the first match, or nil. CRuby additionally sets `$~`
        // / `$1` etc. capture variables; we don't model `$~`, so
        // captures are accessed via `#match` only.
        if &*name == "=~" && args.len() == 1 {
            let result = match (&recv, &args[0]) {
                (Value::Regex(re), Value::Str(s)) | (Value::Str(s), Value::Regex(re)) => {
                    let bound = s.borrow();
                    match re.find(&bound) {
                        Some(m) => Value::Int(m.start() as i64),
                        None => Value::Nil,
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
                Value::Str(s) => Some(self.interner.intern(&s.borrow())),
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
                rescues: vec![],
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
        let kw_count = proto.kw_param_defaults.len();
        // Layout of `m.params` tail:
        //   [...positional..., rest?, ...kw_params..., kw_rest?]
        let positional_max = m.params.len()
            - (if has_rest { 1 } else { 0 })
            - kw_count
            - (if has_kw_rest { 1 } else { 0 });
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
            rescues: vec![],
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
            rescues: vec![],
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
                n_locals: 2,
                code: vec![
                    Op::LoadLocal(0),
                    Op::LoadLocal(1),
                    Op::ApplyCall(call_id, u16::MAX),
                    Op::Return,
                ],
                op_spans: vec![Span::ZERO; 4],
                filename: "<synthetic>".into(),
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
        let rest_array_val = if let Some(slot) = rest_slot {
            let rest_args: Vec<Value> = args.iter().skip(n_params as usize).cloned().collect();
            // Truncate args to the leading required slots — the
            // overflow now lives in rest_args.
            self.maybe_gc();
            self.check_alloc()?;
            let id = self.heap.alloc(HeapObj::Array(rest_args));
            Some((slot, Value::Array(id)))
        } else {
            None
        };
        let proto = &self.protos[proto_idx];
        let needed = proto.n_locals as usize;
        {
            let mut locals = captured.borrow_mut();
            if locals.len() < needed {
                while locals.len() < needed { locals.push(Value::Nil); }
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
            is_block: true, n_given_positional: 0, rescues: vec![],
        });
        Ok(())
    }



    pub(crate) fn do_call_block(&mut self, name_id: SymId, argc: usize, no_recv: bool, cache_id: u16) -> Result<(), Trap> {
        let name = self.interner.resolve(name_id).clone();
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
            if let Some(res) = self.builtin_call(&name, &args) { self.stack.push(res?); return Ok(()); }
            if let Some(host) = self.host_fns.get(&name_id).cloned() {
                let v = self.invoke_host_fn(host, &args)?;
                self.stack.push(v);
                return Ok(());
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
