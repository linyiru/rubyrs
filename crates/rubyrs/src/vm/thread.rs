//! `thread.c` + `thread_sync.c`: native serves for the preamble's
//! hottest `Thread` / `Fiber` / `Mutex` methods (#381).
//!
//! Rails touches `Thread.current` / `Fiber.current` ~35×/request
//! (`IsolatedExecutionState`, `CurrentAttributes`, the logger) and
//! `Mutex#synchronize` on the logger and cache paths. As preamble
//! methods each paid a full frame push (and `synchronize` two more
//! calls plus a `begin`/`ensure`), 6–20× CRuby.
//!
//! Soundness: a serve fires only when dispatch has ALREADY resolved
//! the call to the exact preamble `Method` (Rc identity, captured
//! once after the preamble loads). A user reopen, subclass override
//! or singleton def resolves to a different `Method` and never gets
//! here. Each serve reproduces its preamble body's observable
//! behaviour; any shape outside what it covers returns `Ok(false)`
//! with the stack untouched, and the Ruby body runs.

use std::rc::Rc;

use super::iter::BlockStep;
use super::{PinGuard, Vm};
use crate::error::Trap;
use crate::intern::SymId;
use crate::value::{Class, Method, ObjId, Value};

/// The preamble methods `vm/thread.rs` serves, plus what the serves
/// read. All `None` until `Vm::capture_native_protos` runs.
pub(crate) struct NativeProtos {
    thread_current: Option<Rc<Method>>,
    thread_aref: Option<Rc<Method>>,
    thread_aset: Option<Rc<Method>>,
    fiber_current: Option<Rc<Method>>,
    mutex_synchronize: Option<Rc<Method>>,
    mutex_lock: Option<Rc<Method>>,
    mutex_unlock: Option<Rc<Method>>,
    thread_class: Option<Rc<Class>>,
    mutex_class: Option<Rc<Class>>,
    ivar_coop_current: SymId,
    ivar_fiber_locals: SymId,
    ivar_root_fiber: SymId,
    ivar_owner: SymId,
    ivar_depth: SymId,
    ivar_waiters: SymId,
    /// `method_gen` at which `mutex_ok` was computed.
    mutex_checked_gen: Option<u32>,
    /// `Mutex#lock` / `#unlock` and `Thread.current` still resolve to
    /// the preamble's — the bodies `synchronize` stands in for.
    mutex_ok: bool,
}

impl Default for NativeProtos {
    fn default() -> Self {
        // The ivar syms are never read before capture: every serve
        // first matches a captured `Method`, and those start `None`.
        let unset = SymId(u32::MAX);
        NativeProtos {
            thread_current: None, thread_aref: None, thread_aset: None, fiber_current: None,
            mutex_synchronize: None, mutex_lock: None, mutex_unlock: None,
            thread_class: None, mutex_class: None,
            ivar_coop_current: unset, ivar_fiber_locals: unset, ivar_root_fiber: unset,
            ivar_owner: unset, ivar_depth: unset, ivar_waiters: unset,
            mutex_checked_gen: None, mutex_ok: false,
        }
    }
}

fn same(slot: &Option<Rc<Method>>, m: &Rc<Method>) -> bool {
    slot.as_ref().is_some_and(|s| Rc::ptr_eq(s, m))
}

fn truthy(v: &Value) -> bool {
    !matches!(v, Value::Nil | Value::Bool(false))
}

/// `equal?` for the two shapes a Mutex owner can take: the Thread
/// class (main thread) or a green-thread instance. `None` = some
/// other value, which the serve leaves to the Ruby body.
fn identical(a: &Value, b: &Value) -> Option<bool> {
    match (a, b) {
        (Value::Class(x), Value::Class(y)) => Some(Rc::ptr_eq(x, y)),
        (Value::Object(x), Value::Object(y)) => Some(x == y),
        (Value::Class(_), Value::Object(_)) | (Value::Object(_), Value::Class(_)) => Some(false),
        _ => None,
    }
}

impl Vm {
    /// Record the preamble methods to serve natively. Called from
    /// `load_preamble` on both the cache-hit and live paths.
    pub(crate) fn capture_native_protos(&mut self) {
        let thread = self.classes.get(&self.interner.intern("Thread")).cloned();
        let fiber = self.classes.get(&self.interner.intern("Fiber")).cloned();
        let mutex = self.classes.get(&self.interner.intern("Mutex")).cloned();
        let current = self.interner.intern("current");
        let singleton = |vm: &Self, c: &Option<Rc<Class>>, name: SymId| {
            c.as_ref().and_then(|c| vm.lookup_class_singleton_method(c, name))
        };
        let instance = |vm: &Self, c: &Option<Rc<Class>>, name: &str| {
            let name = vm.interner.get_id(name)?;
            c.as_ref().and_then(|c| vm.lookup_method_uncached(c, name))
        };
        self.native_protos = NativeProtos {
            thread_current: singleton(self, &thread, current),
            thread_aref: singleton(self, &thread, self.sym_index_op),
            thread_aset: singleton(self, &thread, self.sym_index_set_op),
            fiber_current: singleton(self, &fiber, current),
            mutex_synchronize: instance(self, &mutex, "synchronize"),
            mutex_lock: instance(self, &mutex, "lock"),
            mutex_unlock: instance(self, &mutex, "unlock"),
            thread_class: thread,
            mutex_class: mutex,
            ivar_coop_current: self.interner.intern("@coop_current"),
            ivar_fiber_locals: self.interner.intern("@fiber_locals"),
            ivar_root_fiber: self.interner.intern("@root_fiber"),
            ivar_owner: self.interner.intern("@owner"),
            ivar_depth: self.interner.intern("@depth"),
            ivar_waiters: self.interner.intern("@waiters"),
            mutex_checked_gen: None,
            mutex_ok: false,
        };
    }

    /// CRuby's `Thread#[]` store is per FIBER: every fiber starts
    /// with an empty one. The main thread's is `Thread`'s
    /// `@fiber_locals` (preamble/thread.rb), which the serves above
    /// read directly, so `resume_fiber` calls this on entry and again
    /// on exit to swap it with the fiber's own slot. The swap is its
    /// own inverse: while the fiber runs, its slot parks the
    /// resumer's store (and the GC marks it there). A green thread
    /// keeps its store on its Thread instance, so this is a no-op for
    /// what it observes.
    #[cfg(feature = "_fiber")]
    pub(crate) fn swap_fiber_locals(&mut self, fiber: ObjId) {
        let Some(thread) = self.native_protos.thread_class.as_ref() else { return };
        let sym = self.native_protos.ivar_fiber_locals;
        let mut ivars = thread.ivars.borrow_mut();
        let outer = ivars.remove(&sym).unwrap_or(Value::Nil);
        let inner = self.heap.fiber(fiber).fiber_locals.replace(outer);
        if !matches!(inner, Value::Nil) {
            ivars.insert(sym, inner);
        }
    }

    /// `Thread.current`'s body: `@coop_current || self`.
    fn native_thread_current(&self, cls: &Rc<Class>) -> Value {
        match cls.ivars.borrow().get(&self.native_protos.ivar_coop_current) {
            Some(v) if truthy(v) => v.clone(),
            _ => Value::Class(cls.clone()),
        }
    }

    /// Class-receiver serves, called from the class-singleton IC with
    /// the resolved method. Stack: `[.., recv, a1..aN]`, `recv` the
    /// `Value::Class` the method was resolved on (Thread or a
    /// subclass: the bodies read `self`'s own ivars, so do we).
    pub(crate) fn try_serve_native_class_fn(
        &mut self,
        m: &Rc<Method>,
        cls: &Rc<Class>,
        name_id: SymId,
        argc: usize,
    ) -> Result<bool, Trap> {
        let np = &self.native_protos;
        let (fiber_locals, root_fiber) = (np.ivar_fiber_locals, np.ivar_root_fiber);
        let is_current = argc == 0 && same(&np.thread_current, m);
        let is_index = (argc == 1 && same(&np.thread_aref, m)) || (argc == 2 && same(&np.thread_aset, m));
        let is_fiber_current = argc == 0 && same(&np.fiber_current, m);
        if is_current {
            let v = self.native_thread_current(cls);
            self.stack.pop();
            self.stack.push(v);
            return Ok(true);
        }
        if is_index {
            // `@fiber_locals ||= {}; @fiber_locals[key]` (or `[key] =
            // val`): once the store exists this IS a Hash index, so
            // serve it through the same Hash fast path the body's
            // own `@fiber_locals[key]` would take. The first touch
            // (no store yet) runs the Ruby body to create it.
            let Some(Value::Hash(hid)) = cls.ivars.borrow().get(&fiber_locals).cloned() else {
                return Ok(false);
            };
            let recv_idx = self.stack.len() - 1 - argc;
            self.stack[recv_idx] = Value::Hash(hid);
            if self.try_fast_index(name_id, argc, false) {
                return Ok(true);
            }
            self.stack[recv_idx] = Value::Class(cls.clone());
            return Ok(false);
        }
        if is_fiber_current {
            // `(__rubyrs_fiber_current rescue nil) || (@root_fiber ||=
            // Object.new)`; the first root read runs the Ruby body.
            #[cfg(feature = "_fiber")]
            if let Some(id) = self.current_fiber_id {
                self.stack.pop();
                self.stack.push(Value::Object(id));
                return Ok(true);
            }
            let root = match cls.ivars.borrow().get(&root_fiber) {
                Some(v) if truthy(v) => v.clone(),
                _ => return Ok(false),
            };
            self.stack.pop();
            self.stack.push(root);
            return Ok(true);
        }
        Ok(false)
    }

    /// `Mutex#synchronize { }` served natively: lock, run the block,
    /// unlock on every exit (value, `break`, method `return`, raise).
    /// Called from the block-form object IC with the resolved method.
    /// Stack: `[.., recv, block]`.
    ///
    /// Only the uncontended / re-entrant lock on the main thread
    /// outside any fiber: a green thread is a fiber, and a block
    /// driven from Rust cannot be suspended mid-way, so there the
    /// Ruby body (whose `yield` can) keeps the job.
    pub(crate) fn try_serve_mutex_synchronize(
        &mut self,
        m: &Rc<Method>,
        cls: &Rc<Class>,
        id: ObjId,
        block: ObjId,
    ) -> Result<bool, Trap> {
        if !same(&self.native_protos.mutex_synchronize, m) {
            return Ok(false);
        }
        #[cfg(feature = "_fiber")]
        if self.current_fiber_id.is_some() {
            return Ok(false);
        }
        // Exact Mutex (a subclass may override lock/unlock; an
        // eigenclass shows up as a different `cls`).
        if !self.native_protos.mutex_class.as_ref().is_some_and(|c| Rc::ptr_eq(c, cls)) {
            return Ok(false);
        }
        if self.native_protos.mutex_checked_gen != Some(self.method_gen) {
            self.revalidate_native_mutex();
        }
        if !self.native_protos.mutex_ok || self.heap.instance(id).frozen.get() {
            return Ok(false);
        }
        let Some(thread) = self.native_protos.thread_class.clone() else { return Ok(false) };
        let cur = self.native_thread_current(&thread);
        let (owner_sym, depth_sym) = (self.native_protos.ivar_owner, self.native_protos.ivar_depth);
        // `lock`: take it when free, count a re-entry by the owner;
        // anything else (contended, odd ivar state) is the Ruby body's.
        {
            let inst = self.heap.instance(id);
            let owner = inst.ivars.get(cls, owner_sym).cloned().unwrap_or(Value::Nil);
            let depth = inst.ivars.get(cls, depth_sym).cloned().unwrap_or(Value::Nil);
            let waiters_empty = matches!(
                inst.ivars.get(cls, self.native_protos.ivar_waiters),
                Some(Value::Array(w)) if self.heap.array(*w).is_empty()
            );
            let (Value::Int(d), true) = (depth, waiters_empty) else { return Ok(false) };
            let write = match &owner {
                Value::Nil => (owner_sym, cur.clone()),
                o => match identical(o, &cur) {
                    Some(true) => (depth_sym, Value::Int(d + 1)),
                    _ => return Ok(false),
                },
            };
            self.heap.instance_mut(id).ivars.insert(cls, write.0, write.1);
        }
        self.stack.pop(); // block
        let recv = self.stack.pop().unwrap_or(Value::Nil);
        let pre_frames = self.frames.len();
        let mut g = PinGuard::new(self);
        g.pin(recv.clone());
        g.pin(Value::Block(block));
        let r = g.vm.step_block(block, Vec::new(), pre_frames);
        // `ensure unlock` — its own error replaces the block's outcome,
        // as a raising `ensure` clause does.
        g.vm.native_mutex_unlock(recv, cls, id, &cur)?;
        let v = match r? {
            // `method_return` stays set; the outer dispatch unwinds on it.
            BlockStep::MethodReturn => Value::Nil,
            BlockStep::Break(v) | BlockStep::Value(v) => v,
        };
        g.vm.stack.push(v);
        Ok(true)
    }

    fn revalidate_native_mutex(&mut self) {
        let np = &self.native_protos;
        let ok = match (&np.mutex_class, &np.thread_class) {
            (Some(mc), Some(tc)) => {
                let resolves = |slot: &Option<Rc<Method>>, found: Option<Rc<Method>>| {
                    found.is_some_and(|f| same(slot, &f))
                };
                let (lock, unlock) = (self.interner.get_id("lock"), self.interner.get_id("unlock"));
                let current = self.interner.get_id("current");
                lock.is_some_and(|s| resolves(&np.mutex_lock, self.lookup_method_uncached(mc, s)))
                    && unlock.is_some_and(|s| resolves(&np.mutex_unlock, self.lookup_method_uncached(mc, s)))
                    && current.is_some_and(|s| resolves(&np.thread_current, self.lookup_class_singleton_method(tc, s)))
            }
            _ => false,
        };
        self.native_protos.mutex_ok = ok;
        self.native_protos.mutex_checked_gen = Some(self.method_gen);
    }

    /// `Mutex#unlock` for the served lock. A waiter queued while the
    /// block ran (a green thread started inside it) needs the
    /// scheduler's wake-up, so that case calls the Ruby `unlock`.
    fn native_mutex_unlock(&mut self, recv: Value, cls: &Rc<Class>, id: ObjId, cur: &Value) -> Result<(), Trap> {
        let (owner_sym, depth_sym) = (self.native_protos.ivar_owner, self.native_protos.ivar_depth);
        let inst = self.heap.instance(id);
        let waiters_empty = matches!(
            inst.ivars.get(cls, self.native_protos.ivar_waiters),
            Some(Value::Array(w)) if self.heap.array(*w).is_empty()
        );
        if !waiters_empty {
            let Some(unlock) = self.native_protos.mutex_unlock.clone() else { return Ok(()) };
            // A pending method `return` would short-circuit the nested
            // dispatch; park it across the call.
            let mr = self.method_return.take();
            let pre_frames = self.frames.len();
            let r = self.invoke_method(unlock, recv, Vec::new()).and_then(|()| self.dispatch_until(pre_frames));
            self.method_return = mr;
            r?;
            self.stack.pop();
            return Ok(());
        }
        let owner = inst.ivars.get(cls, owner_sym).cloned().unwrap_or(Value::Nil);
        if identical(&owner, cur) != Some(true) {
            return Ok(()); // unlocked inside the block: `unlock` is a no-op
        }
        let write = match inst.ivars.get(cls, depth_sym) {
            Some(Value::Int(d)) if *d > 0 => (depth_sym, Value::Int(d - 1)),
            _ => (owner_sym, Value::Nil),
        };
        self.heap.instance_mut(id).ivars.insert(cls, write.0, write.1);
        Ok(())
    }
}
