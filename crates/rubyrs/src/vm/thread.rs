//! Thread / Fiber / Mutex intrinsics. Mirrors CRuby's `thread.c`
//! (`Thread.current`, the fiber-local `Thread#[]` / `#[]=`) and
//! `thread_sync.c` (`Mutex#synchronize`).
//!
//! The semantics stay in the preamble (preamble/thread.rb,
//! preamble/mutex.rb); this module only serves the hot, uncontended
//! shapes without a Ruby frame, and only while the preamble method a
//! call site resolved to is still the one captured at boot
//! (`Rc::ptr_eq`, the `rtm_default_stub` precedent). A user
//! redefinition installs a new `Rc<Method>`, so it can never match.
//! Every serve here is observably identical to running the preamble
//! body, except that `Mutex#synchronize` no longer shows up as a
//! backtrace frame (CRuby shows it as a C frame at the caller's
//! line; rubyrs showed a preamble line; now neither).
//!
//! Contents:
//!   - `ThreadIntrinsics` — the captured methods and pre-interned
//!     ivar names, filled by `cache_thread_intrinsics` at
//!     `load_preamble` time.
//!   - `Vm::try_thread_class_intrinsic` — `Thread.current`,
//!     `Thread.current[:k]`, `Thread.current[:k] = v`,
//!     `Fiber.current` (called from `try_invoke_class_singleton_cached`).
//!   - `Vm::try_mutex_synchronize` — native lock / yield /
//!     ensure-unlock (called from `try_invoke_explicit_recv_block_cached`).
//!   - `Vm::fiber_locals_value` — the per-fiber fiber-local Hash
//!     behind the `__rubyrs_fiber_locals` kernel builtin.

use std::rc::Rc;

use crate::error::Trap;
use crate::heap::HeapObj;
use crate::intern::{Interner, SymId};
use crate::value::{Class, Method, ObjId, Value};

use super::iter::BlockStep;
use super::{PinGuard, Vm};

pub(crate) struct ThreadIntrinsics {
    thread_class: Option<Rc<Class>>,
    mutex_class: Option<Rc<Class>>,
    thread_current: Option<Rc<Method>>,
    thread_aref: Option<Rc<Method>>,
    thread_aset: Option<Rc<Method>>,
    fiber_current: Option<Rc<Method>>,
    mutex_synchronize: Option<Rc<Method>>,
    mutex_lock: Option<Rc<Method>>,
    mutex_unlock: Option<Rc<Method>>,
    /// `method_gen` at which `mutex_intact` was last computed. The
    /// native synchronize inlines `lock`, `unlock`, and the
    /// `::Thread.current` they call, so all three must still resolve
    /// to the preamble's methods.
    mutex_checked_gen: Option<u32>,
    mutex_intact: bool,
    sym_coop_current: SymId,
    sym_fiber_locals: SymId,
    sym_root_fiber: SymId,
    sym_owner: SymId,
    sym_depth: SymId,
    sym_waiters: SymId,
    sym_lock: SymId,
    sym_unlock: SymId,
    sym_current: SymId,
}

impl ThreadIntrinsics {
    pub(crate) fn new(interner: &mut Interner) -> Self {
        ThreadIntrinsics {
            thread_class: None,
            mutex_class: None,
            thread_current: None,
            thread_aref: None,
            thread_aset: None,
            fiber_current: None,
            mutex_synchronize: None,
            mutex_lock: None,
            mutex_unlock: None,
            mutex_checked_gen: None,
            mutex_intact: false,
            sym_coop_current: interner.intern("@coop_current"),
            sym_fiber_locals: interner.intern("@fiber_locals"),
            sym_root_fiber: interner.intern("@root_fiber"),
            sym_owner: interner.intern("@owner"),
            sym_depth: interner.intern("@depth"),
            sym_waiters: interner.intern("@waiters"),
            sym_lock: interner.intern("lock"),
            sym_unlock: interner.intern("unlock"),
            sym_current: interner.intern("current"),
        }
    }
}

fn is_method(slot: &Option<Rc<Method>>, m: &Rc<Method>) -> bool {
    slot.as_ref().is_some_and(|c| Rc::ptr_eq(c, m))
}

/// `equal?` for the two shapes a Mutex owner can take: the Thread
/// class itself (the main thread) or a green-thread instance.
fn same_thread(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Class(x), Value::Class(y)) => Rc::ptr_eq(x, y),
        (Value::Object(x), Value::Object(y)) => x == y,
        _ => false,
    }
}

impl Vm {
    /// Capture the preamble's Thread / Fiber / Mutex methods. Called
    /// from `load_preamble` on both the preamble-cache hit and miss
    /// paths, before any user code runs.
    pub(crate) fn cache_thread_intrinsics(&mut self) {
        let class = |vm: &mut Vm, name: &str| {
            let sym = vm.interner.intern(name);
            vm.classes.get(&sym).cloned()
        };
        let thread = class(self, "Thread");
        let fiber = class(self, "Fiber");
        let mutex = class(self, "Mutex");
        let singleton = |vm: &mut Vm, cls: &Option<Rc<Class>>, name: &str| {
            let sym = vm.interner.intern(name);
            cls.as_ref().and_then(|c| vm.lookup_class_singleton_method(c, sym))
        };
        self.thread_intr.thread_current = singleton(self, &thread, "current");
        self.thread_intr.thread_aref = singleton(self, &thread, "[]");
        self.thread_intr.thread_aset = singleton(self, &thread, "[]=");
        self.thread_intr.fiber_current = singleton(self, &fiber, "current");
        let instance = |vm: &mut Vm, name: &str| {
            let sym = vm.interner.intern(name);
            mutex.as_ref().and_then(|c| vm.lookup_method_uncached(c, sym))
        };
        self.thread_intr.mutex_synchronize = instance(self, "synchronize");
        self.thread_intr.mutex_lock = instance(self, "lock");
        self.thread_intr.mutex_unlock = instance(self, "unlock");
        self.thread_intr.thread_class = thread;
        self.thread_intr.mutex_class = mutex;
        self.thread_intr.mutex_checked_gen = None;
    }

    /// The fiber-local store `Thread.current[..]` resolves to on the
    /// class-level (main-thread) receiver `cls`, mirroring preamble
    /// `Thread.__fiber_local_store`: the running fiber's own Hash when
    /// inside a non-root fiber of the main thread, else the class's
    /// `@fiber_locals`. `Some(None)` = that store is not allocated
    /// yet; `None` = an unexpected shape, take the Ruby path.
    fn fiber_local_store(&self, cls: &Rc<Class>) -> Option<Option<ObjId>> {
        let ivars = cls.ivars.borrow();
        #[cfg(feature = "_fiber")]
        if let Some(fid) = self.current_fiber_id
            && !ivars.get(&self.thread_intr.sym_coop_current).is_some_and(Value::is_truthy)
        {
            return match &*self.heap.fiber(fid).locals.borrow() {
                Value::Hash(h) => Some(Some(*h)),
                Value::Nil => Some(None),
                _ => None,
            };
        }
        match ivars.get(&self.thread_intr.sym_fiber_locals) {
            Some(Value::Hash(h)) => Some(Some(*h)),
            None | Some(Value::Nil) => Some(None),
            _ => None,
        }
    }

    /// Frameless serve of `Thread.current`, `Thread.current[:k]`,
    /// `Thread.current[:k] = v` and `Fiber.current` when `m` (already
    /// resolved and arity-checked by the caller) is the captured
    /// preamble method. Stack layout `[.., recv, a1, .., aN]`;
    /// `Ok(false)` leaves it untouched.
    pub(crate) fn try_thread_class_intrinsic(
        &mut self,
        m: &Rc<Method>,
        cls: &Rc<Class>,
        argc: usize,
        recv_idx: usize,
    ) -> Result<bool, Trap> {
        let ti = &self.thread_intr;
        if is_method(&ti.thread_current, m) {
            // `@coop_current || self`
            let v = cls.ivars.borrow().get(&ti.sym_coop_current).filter(|v| v.is_truthy()).cloned();
            self.stack.truncate(recv_idx);
            self.stack.push(v.unwrap_or_else(|| Value::Class(cls.clone())));
            return Ok(true);
        }
        let aref = is_method(&ti.thread_aref, m);
        if aref || is_method(&ti.thread_aset, m) {
            // Symbol keys only (a String key needs `to_sym`, anything
            // else raises); `[k] = nil` deletes, which the Ruby path owns.
            if !matches!(self.stack[recv_idx + 1], Value::Sym(_))
                || (!aref && matches!(self.stack[recv_idx + 2], Value::Nil))
            {
                return Ok(false);
            }
            let Some(store) = self.fiber_local_store(cls) else { return Ok(false) };
            let Some(h) = store else {
                if !aref {
                    return Ok(false); // first write allocates the store
                }
                self.stack.truncate(recv_idx);
                self.stack.push(Value::Nil);
                return Ok(true);
            };
            // A per-instance eigenclass on the store (`def h.[]`)
            // overrides everything; the generic dispatch probes it
            // before `try_fast_index`, so this serve must too.
            if self.any_hash_singletons
                && matches!(self.heap.get(h), HeapObj::Hash(hh) if hh.singleton_class().is_some())
            {
                return Ok(false);
            }
            // Swap the receiver for the store and run the plain-Hash
            // `[]` / `[]=` fast path; it declines (defaulted / frozen /
            // user-patched Hash) with the stack unchanged.
            let recv = std::mem::replace(&mut self.stack[recv_idx], Value::Hash(h));
            let op = if aref { self.sym_index_op } else { self.sym_index_set_op };
            if self.try_fast_index(op, argc, false) {
                return Ok(true);
            }
            self.stack[recv_idx] = recv;
            return Ok(false);
        }
        if is_method(&ti.fiber_current, m) {
            #[cfg(feature = "_fiber")]
            if let Some(fid) = self.current_fiber_id {
                self.stack.truncate(recv_idx);
                self.stack.push(Value::Object(fid));
                return Ok(true);
            }
            // Root fiber: the preamble's `@root_fiber ||= Object.new`,
            // once it has been allocated.
            let root = cls.ivars.borrow().get(&ti.sym_root_fiber).filter(|v| v.is_truthy()).cloned();
            if let Some(v) = root {
                self.stack.truncate(recv_idx);
                self.stack.push(v);
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn mutex_intact(&mut self) -> bool {
        if self.thread_intr.mutex_checked_gen == Some(self.method_gen) {
            return self.thread_intr.mutex_intact;
        }
        let ti = &self.thread_intr;
        let ok = match (&ti.mutex_class, &ti.thread_class) {
            (Some(mutex), Some(thread)) => {
                let resolves = |slot: &Option<Rc<Method>>, found: Option<Rc<Method>>| {
                    found.is_some_and(|f| is_method(slot, &f))
                };
                resolves(&ti.mutex_lock, self.lookup_method_uncached(mutex, ti.sym_lock))
                    && resolves(&ti.mutex_unlock, self.lookup_method_uncached(mutex, ti.sym_unlock))
                    && resolves(&ti.thread_current, self.lookup_class_singleton_method(thread, ti.sym_current))
            }
            _ => false,
        };
        self.thread_intr.mutex_intact = ok;
        self.thread_intr.mutex_checked_gen = Some(self.method_gen);
        ok
    }

    /// `::Thread.current` for a native Mutex serve of receiver class
    /// `cls`, or `None` when the serve must decline: inside a fiber
    /// (a `Fiber.yield` under a native driver cannot be stashed, see
    /// `resume_native_iter_depth`), for a Mutex subclass or singleton,
    /// and once `lock` / `unlock` / `Thread.current` are patched.
    fn mutex_serve_thread(&mut self, cls: &Rc<Class>) -> Option<Value> {
        #[cfg(feature = "_fiber")]
        if self.current_fiber_id.is_some() {
            return None;
        }
        if !self.thread_intr.mutex_class.as_ref().is_some_and(|c| Rc::ptr_eq(c, cls)) || !self.mutex_intact() {
            return None;
        }
        let thread = self.thread_intr.thread_class.clone()?;
        let cur = thread.ivars.borrow().get(&self.thread_intr.sym_coop_current).filter(|v| v.is_truthy()).cloned();
        Some(cur.unwrap_or(Value::Class(thread)))
    }

    /// Preamble `Mutex#lock`, uncontended: take a free lock or count a
    /// re-entry. `false` (nothing mutated) for a lock held by another
    /// green thread, where the Ruby `lock` parks, and for a frozen or
    /// unexpectedly shaped Mutex.
    fn mutex_lock_native(&mut self, id: ObjId, cur: &Value) -> bool {
        let (sym_owner, sym_depth) = (self.thread_intr.sym_owner, self.thread_intr.sym_depth);
        let (owner, depth) = match self.heap.get(id) {
            HeapObj::Instance(i) if !i.frozen.get() => match (i.ivar_get(sym_owner), i.ivar_get(sym_depth)) {
                (Some(o), Some(Value::Int(d))) => (o.clone(), *d),
                _ => return false,
            },
            _ => return false,
        };
        if matches!(owner, Value::Nil) {
            self.heap.instance_mut(id).ivar_set(sym_owner, cur.clone());
        } else if same_thread(&owner, cur) && depth < i64::MAX {
            self.heap.instance_mut(id).ivar_set(sym_depth, Value::Int(depth + 1));
        } else {
            return false;
        }
        true
    }

    /// Preamble `Mutex#unlock` without its wake step: a no-op unless
    /// `cur` owns the lock, else drop one re-entry level or release.
    /// `false` (nothing mutated) when the Ruby `unlock` must run: a
    /// release with parked waiters (it wakes one through the coop
    /// scheduler), and a frozen or unexpectedly shaped Mutex.
    fn mutex_unlock_native(&mut self, id: ObjId, cur: &Value) -> bool {
        let ti = &self.thread_intr;
        let (sym_owner, sym_depth, sym_waiters) = (ti.sym_owner, ti.sym_depth, ti.sym_waiters);
        let (owner, depth, waiters) = match self.heap.get(id) {
            HeapObj::Instance(i) if !i.frozen.get() => (
                i.ivar_get(sym_owner).cloned(),
                i.ivar_get(sym_depth).cloned(),
                i.ivar_get(sym_waiters).cloned(),
            ),
            _ => return false,
        };
        if !owner.is_some_and(|o| same_thread(&o, cur)) {
            return true;
        }
        match (depth, waiters) {
            (Some(Value::Int(d)), _) if d > 0 => {
                self.heap.instance_mut(id).ivar_set(sym_depth, Value::Int(d - 1));
                true
            }
            (Some(Value::Int(_)), Some(Value::Array(w))) if self.heap.array(w).is_empty() => {
                self.heap.instance_mut(id).ivar_set(sym_owner, Value::Nil);
                true
            }
            _ => false,
        }
    }

    /// Native `Mutex#lock` / `Mutex#unlock` (no block; stack layout
    /// `[.., recv]`), both returning the receiver. Same declines as
    /// `mutex_serve_thread` plus the `_native` helpers' own.
    pub(crate) fn try_mutex_lock_unlock(
        &mut self,
        m: &Rc<Method>,
        cls: &Rc<Class>,
        id: ObjId,
        argc: usize,
    ) -> bool {
        let lock = is_method(&self.thread_intr.mutex_lock, m);
        if argc != 0 || !(lock || is_method(&self.thread_intr.mutex_unlock, m)) {
            return false;
        }
        let Some(cur) = self.mutex_serve_thread(cls) else { return false };
        // The receiver slot already holds `self`, the return value.
        if lock { self.mutex_lock_native(id, &cur) } else { self.mutex_unlock_native(id, &cur) }
    }

    /// Native `Mutex#synchronize` for the uncontended case: lock, run
    /// the block, and unlock on every exit (value, `break`, `return`,
    /// `throw`, exception) exactly as preamble/mutex.rb's
    /// `lock; begin; yield; ensure; unlock; end` does. Stack layout
    /// `[.., recv, block]`; declines (stack untouched) as
    /// `mutex_serve_thread` / `mutex_lock_native` do.
    pub(crate) fn try_mutex_synchronize(
        &mut self,
        m: &Rc<Method>,
        cls: &Rc<Class>,
        id: ObjId,
        block_id: ObjId,
        argc: usize,
    ) -> Result<bool, Trap> {
        if argc != 0 || !is_method(&self.thread_intr.mutex_synchronize, m) {
            return Ok(false);
        }
        let Some(cur) = self.mutex_serve_thread(cls) else { return Ok(false) };
        if !self.mutex_lock_native(id, &cur) {
            return Ok(false);
        }
        // `[.., recv, block]` -> `[..]`; the block runs as a native
        // driver's block (no synchronize frame).
        let recv_idx = self.stack.len() - 2;
        self.stack.truncate(recv_idx);
        let pre_frames = self.frames.len();
        let mut g = PinGuard::new(self);
        g.pin(Value::Object(id));
        g.pin(Value::Block(block_id));
        g.pin(cur.clone());
        let step = g.vm.step_block(block_id, Vec::new(), pre_frames);
        // The block's value is unrooted while a Ruby-level unlock runs
        // (it can collect).
        if let Ok(BlockStep::Value(v) | BlockStep::Break(v)) = &step {
            g.pin(v.clone());
        }
        // The block may have redefined `unlock` / `Thread.current` or
        // given the receiver a singleton class; Ruby's `ensure; unlock`
        // would see that, so re-check before releasing natively.
        let native = g.vm.heap.class_of(id);
        let unlocked = match g.vm.mutex_serve_thread(&native) {
            Some(cur) if g.vm.mutex_unlock_native(id, &cur) => Ok(()),
            _ => g.vm.mutex_unlock_ruby(id),
        };
        drop(g);
        let v = match step? {
            BlockStep::Value(v) | BlockStep::Break(v) => v,
            // `method_return` stays set; the outer dispatch unwinds.
            BlockStep::MethodReturn => Value::Nil,
        };
        unlocked?;
        self.stack.push(v);
        Ok(true)
    }

    /// Run the preamble `Mutex#unlock` synchronously. It may run while
    /// a `return` / `break` signal is pending (the block exited that
    /// way), so those are parked around the call and restored after,
    /// as Ruby's own `ensure` does. `unlock` is resolved afresh on the
    /// receiver, so a redefinition made inside the block is the one run.
    fn mutex_unlock_ruby(&mut self, id: ObjId) -> Result<(), Trap> {
        let cls = self.heap.class_of(id);
        let found = self.lookup_method_uncached(&cls, self.thread_intr.sym_unlock);
        let Some(unlock) = found.or_else(|| self.thread_intr.mutex_unlock.clone()) else { return Ok(()) };
        let method_return = self.method_return.take();
        let method_return_locals = self.method_return_locals.take();
        let break_signaled = std::mem::replace(&mut self.break_signaled, false);
        let pre_frames = self.frames.len();
        let r = self.invoke_method(unlock, Value::Object(id), Vec::new()).and_then(|()| self.dispatch_until(pre_frames));
        if r.is_ok() {
            self.stack.pop();
        }
        self.method_return = method_return;
        self.method_return_locals = method_return_locals;
        self.break_signaled = break_signaled;
        r.map(|_| ())
    }

    /// `__rubyrs_fiber_locals` — the running non-root fiber's
    /// fiber-local Hash, allocated on first use; `nil` on the root
    /// fiber (and in builds without fibers), where preamble
    /// `Thread.__fiber_local_store` falls back to the class's
    /// process-global `@fiber_locals`.
    pub(crate) fn fiber_locals_value(&mut self) -> Result<Value, Trap> {
        #[cfg(feature = "_fiber")]
        if let Some(fid) = self.current_fiber_id {
            let cur = self.heap.fiber(fid).locals.borrow().clone();
            if let Value::Hash(_) = cur {
                return Ok(cur);
            }
            let mut g = PinGuard::new(self);
            g.pin(Value::Object(fid));
            g.vm.maybe_gc();
            g.vm.check_alloc()?;
            let h = g.vm.heap.alloc(HeapObj::Hash(crate::heap::HashObj::with_pairs(Vec::new())));
            // `get_mut` records the write barrier (an old fiber now
            // holds a young Hash).
            if let HeapObj::Fiber(f) = g.vm.heap.get_mut(fid) {
                *f.locals.get_mut() = Value::Hash(h);
            }
            return Ok(Value::Hash(h));
        }
        Ok(Value::Nil)
    }
}

#[cfg(test)]
mod tests {
    use crate::value::Value;

    fn eval_str(src: &str) -> String {
        let mut rt = crate::Runtime::new();
        match rt.eval(src, "thread_intrinsics.rb").expect("eval ok") {
            Value::Str(s) => s.to_string_lossy().to_string(),
            other => panic!("expected Str, got {other:?}"),
        }
    }

    /// A redefinition of `Thread.[]` / `Thread.current` must win over
    /// the native serve (the captured-method identity check fails).
    #[test]
    fn user_thread_overrides_win() {
        let out = eval_str(r##"
            Thread.current[:a] = 1
            class Thread
              class << self
                alias_method :orig_aref, :[]
                def [](k) = [:patched, orig_aref(k)]
              end
            end
            r = [Thread[:a]]
            class Thread
              def self.current = :cur
            end
            r << Thread.current
            r.inspect
        "##);
        assert_eq!(out, "[[:patched, 1], :cur]");
    }

    /// A per-instance override on the backing store Hash is honoured
    /// by both `Thread.current[:k]` and `Thread.current[:k] = v`.
    #[test]
    fn store_hash_singleton_override_wins() {
        let out = eval_str(r##"
            Thread.current[:a] = 1
            h = Thread.instance_variable_get(:@fiber_locals)
            def h.[](k) = [:patched, k]
            def h.[]=(k, v); super(k, [:wrapped, v]); end
            Thread.current[:b] = 2
            [Thread.current[:a], h.fetch(:b)].inspect
        "##);
        assert_eq!(out, "[[:patched, :a], [:wrapped, 2]]");
    }

    /// rubyrs's `synchronize` runs Ruby-level `lock`/`unlock`
    /// overrides (CRuby's C implementation does not; a deliberate
    /// divergence, see preamble/mutex.rb), so a subclass or singleton
    /// override must take the Ruby path, never the native one.
    #[test]
    fn mutex_lock_unlock_overrides_are_called() {
        let out = eval_str(r##"
            $log = []
            class LoudMutex < Mutex
              def lock = ($log << :lock; super)
            end
            lm = LoudMutex.new
            $log << lm.synchronize { :sub } << lm.locked?
            m = Mutex.new
            def m.unlock = ($log << :unlock; super)
            $log << m.synchronize { :single } << m.locked?
            m2 = Mutex.new
            $log << m2.synchronize { def m2.unlock = ($log << :late; super); :late_def } << m2.locked?
            class Mutex
              def lock = ($log << :global; @owner = Thread.current; self)
            end
            $log << Mutex.new.synchronize { :g }
            $log.inspect
        "##);
        assert_eq!(out, "[:lock, :sub, false, :unlock, :single, false, :late, :late_def, false, :global, :g]");
    }
}
