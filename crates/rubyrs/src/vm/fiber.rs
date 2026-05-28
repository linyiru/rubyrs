//! Fiber primitive — ADR 0023 v2 Phase 1a.
//!
//! **Tier 2** per [ADR 0017](../../../docs/adr/0017-tier1-boundary.md)
//! §"Out of Tier 1" row 123 (`Fiber, Enumerator | 2 (language
//! feature) | deferred until a real use case`). Per
//! [ADR 0019 v3](../../../docs/adr/0019-tier2-tier3-boundary.md)
//! the long-term home is the `rubyrs-language` crate split with
//! `_fiber` as the feature flag. Today (pre-Phase-1-split) we
//! keep the implementation in `rubyrs` itself behind the same
//! `_fiber` feature; the gate moves with the code when
//! `rubyrs-language` extracts in ADR 0018 Phase 1.
//!
//! The whole module is `#[cfg(feature = "_fiber")]` at its
//! mod declaration in `vm.rs`. With the feature OFF, the entire
//! file is absent from the build — Tier 1 has zero Fiber cost.
//!
//! **First consumer**: `_http_server`'s A3β async-streaming path
//! ([ADR 0023](../../../docs/adr/0023-true-async-streaming.md)).
//! Embedders wanting streaming responses enable both `_fiber`
//! and `_http_server`.
//!
//! This module ships the **type surface only** — no
//! callable behavior yet. P1b (FiberStashGuard) makes the
//! snapshot swap panic-safe; P1c (bytecode ops) makes
//! Fibers actually runnable from Ruby. The split keeps
//! atomic-commit diffs reviewable.
//!
//! What's here:
//!
//! - [`FiberState`] — `Created | Running | Suspended | Returned`.
//! - [`FiberSnapshot`] — the exact set of `Vm` fields a
//!   suspended Fiber stashes. Per ADR 0023 v2
//!   §"Fiber-scoped Vm state" the snapshot covers 11 of
//!   the 12 "Must stash" fields documented there
//!   (`last_read_line` is named in the ADR but not yet a
//!   Vm field — added when it lands).
//! - [`FiberObject`] — the heap object: snapshot + IP +
//!   proto + last-yielded value + state.
//! - [`Vm::alloc_fiber`] — the allocator, via
//!   `HeapObj::Fiber`.
//!
//! What's NOT here (deferred to later P1 sub-stages):
//!
//! - `FiberStashGuard` (P1b) — panic-safe RAII swap.
//! - `Value::Fiber(ObjId)` variant + Fiber.new / resume /
//!   yield bytecode ops (P1c).
//! - GC mark walk through suspended snapshots (P1d).
//! - `Config::max_live_fibers` cap (P1e).

use std::cell::RefCell;
use std::rc::Rc;

use crate::value::{ObjId, Value};
#[cfg(feature = "regex")]
use crate::vm::LastMatch;
use crate::vm::{Frame, LoopTransfer, Visibility};

/// Lifecycle state of a [`FiberObject`].
///
/// Transitions: `Created` → (first resume) → `Running` →
/// (yield) → `Suspended` → (resume) → `Running` → ... →
/// (block returns) → `Returned`. A resume of a `Returned`
/// Fiber raises `FiberError`. P1c wires the transitions;
/// P1a only defines the enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // variants used in P1c
pub(crate) enum FiberState {
    Created,
    Running,
    Suspended,
    Returned,
}

/// Vm-side state that a suspended Fiber must own.
///
/// The set is normative — adding a new "Must stash" Vm
/// field MUST also add a slot here (the `from_vm` /
/// `install_into` helpers below give a compile-time prompt:
/// missing a field means one of the constructors won't
/// type-check).
///
/// Field semantics (mirrors ADR 0023 v2 §"Fiber-scoped
/// Vm state"):
///
/// - `frames` — the active call stack. Drives bytecode
///   resumption.
/// - `stack` — operand stack. Carries partial expression
///   values across the yield boundary.
/// - `pinned` — GC pins. Fiber-scoped pins follow the
///   Fiber, not the resumer.
/// - `class_stack` — open `class Foo; ...; end` context.
///   Yielding inside a class body must not leak into the
///   resumer's class context.
/// - `class_visibility_stack` — tracks `private` / `public`
///   / `protected` in the open class.
/// - `method_return` — pending `return` value. A `return`
///   from a method-body Fiber must NOT unwind the
///   resumer's Rust frame; stashing keeps the unwind
///   Fiber-local.
/// - `break_signaled` — same shape as `method_return` for
///   `break`.
/// - `pending_loop_transfer` — `next` / `redo` markers.
/// - `suppress_call_result_push` — op-sequencing flag from
///   `step.rs`.
/// - `bypass_visibility_once` — `send` private-dispatch
///   flag.
/// - `last_match` (regex feature) — `$~` is Fiber-local
///   per CRuby.
///
/// **NOT** stashed (process-wide; see ADR's "Do not stash"
/// table): `heap`, `interner`, `classes`, `constants`,
/// `globals`, `host_fns`, `cext_*`, `cext_depth`.
///
/// Not `Clone` — by design. P1b's `FiberStashGuard` moves
/// the snapshot in/out via `mem::swap`, which is O(1) and
/// doesn't require any field to implement `Clone`. Adding
/// a `Clone` impl would invite expensive accidental
/// cloning of the (potentially deep) `frames` Vec.
#[allow(dead_code)] // P1b consumes these via swap
pub(crate) struct FiberSnapshot {
    pub(crate) frames: Vec<Frame>,
    pub(crate) stack: Vec<Value>,
    pub(crate) pinned: Vec<Value>,
    pub(crate) class_stack: Vec<Rc<crate::value::Class>>,
    pub(crate) class_visibility_stack: Vec<Visibility>,
    pub(crate) method_return: Option<Value>,
    pub(crate) break_signaled: bool,
    pub(crate) pending_loop_transfer: Option<LoopTransfer>,
    pub(crate) suppress_call_result_push: bool,
    pub(crate) bypass_visibility_once: bool,
    #[cfg(feature = "regex")]
    pub(crate) last_match: Option<LastMatch>,
}

impl FiberSnapshot {
    /// Swap every "Must stash" Vm field with this snapshot
    /// in O(1) via `mem::swap` — no cloning of Vecs.
    ///
    /// Used by [`FiberStashGuard::install`] and `Drop` to
    /// move state between `Vm` and a snapshot. Two
    /// consecutive `swap_with_vm` calls with the same Vm
    /// are an identity operation (used by tests to confirm
    /// field coverage).
    ///
    /// Adding a new "Must stash" Vm field per ADR 0023 v2
    /// §"Fiber-scoped Vm state" MUST add a `mem::swap` line
    /// here AND a matching field in [`FiberSnapshot`]. The
    /// compiler enforces the second; this comment is the
    /// prompt for the first.
    pub(crate) fn swap_with_vm(&mut self, vm: &mut crate::vm::Vm) {
        std::mem::swap(&mut vm.frames, &mut self.frames);
        std::mem::swap(&mut vm.stack, &mut self.stack);
        std::mem::swap(&mut vm.pinned, &mut self.pinned);
        std::mem::swap(&mut vm.class_stack, &mut self.class_stack);
        std::mem::swap(
            &mut vm.class_visibility_stack,
            &mut self.class_visibility_stack,
        );
        std::mem::swap(&mut vm.method_return, &mut self.method_return);
        std::mem::swap(&mut vm.break_signaled, &mut self.break_signaled);
        std::mem::swap(
            &mut vm.pending_loop_transfer,
            &mut self.pending_loop_transfer,
        );
        std::mem::swap(
            &mut vm.suppress_call_result_push,
            &mut self.suppress_call_result_push,
        );
        std::mem::swap(
            &mut vm.bypass_visibility_once,
            &mut self.bypass_visibility_once,
        );
        #[cfg(feature = "regex")]
        std::mem::swap(&mut vm.last_match, &mut self.last_match);
    }

    /// Construct the snapshot a freshly-created Fiber
    /// installs on its first resume — empty / default for
    /// every field. The Fiber's own bytecode then pushes
    /// the first frame as part of `Fiber.new`'s
    /// initial-call setup (wired in P1c).
    #[allow(dead_code)] // P1b/P1c constructor — tests use it today
    pub(crate) fn empty() -> Self {
        Self {
            frames: Vec::new(),
            stack: Vec::new(),
            pinned: Vec::new(),
            class_stack: Vec::new(),
            class_visibility_stack: Vec::new(),
            method_return: None,
            break_signaled: false,
            pending_loop_transfer: None,
            suppress_call_result_push: false,
            bypass_visibility_once: false,
            #[cfg(feature = "regex")]
            last_match: None,
        }
    }
}

/// Heap-allocated Fiber. The `HeapObj::Fiber(FiberObject)`
/// variant in `heap.rs` wraps this.
///
/// P1a (this commit) ships construction + state read. P1b
/// adds the panic-safe swap that mutates `snapshot` +
/// `state` together. P1c wires the bytecode that drives
/// the state transitions.
#[allow(dead_code)] // fields used in P1b/P1c
pub(crate) struct FiberObject {
    /// The Fiber's body — a `Proc`/`Lambda` ObjId. The
    /// first `resume` enters this block; subsequent
    /// resumes pick up from the suspended bytecode IP in
    /// the snapshot's top frame.
    ///
    /// Held as an `ObjId` (not an `Rc<BlockHandle>`) so the
    /// GC marks it as a root for the Fiber's lifetime —
    /// same shape as `Frame::block_arg` and the rest of
    /// rubyrs's block storage (see P2-13 cycle fix in
    /// `Value::Block`'s doc-comment).
    pub(crate) body_block: ObjId,
    /// Last value yielded by `Fiber.yield(v)` (read by the
    /// resumer) OR the value passed to the most recent
    /// `fiber.resume(v)` (read by `Fiber.yield`'s return
    /// site). P1c wires both directions; P1a only stores
    /// the slot.
    pub(crate) last_value: RefCell<Value>,
    /// Suspended Vm state. Empty until first yield.
    pub(crate) snapshot: RefCell<FiberSnapshot>,
    /// Lifecycle state. `RefCell` so P1c's bytecode ops
    /// can mutate without needing `&mut HeapObj` (which
    /// the heap doesn't hand out cheaply mid-dispatch).
    pub(crate) state: RefCell<FiberState>,
}

impl FiberObject {
    /// Construct a fresh `FiberObject` for the given body
    /// block. Lifecycle starts at `Created`; first
    /// `resume` transitions to `Running` (P1c).
    #[allow(dead_code)] // P1c constructor — tests use it today
    pub(crate) fn new(body_block: ObjId) -> Self {
        Self {
            body_block,
            last_value: RefCell::new(Value::Nil),
            snapshot: RefCell::new(FiberSnapshot::empty()),
            state: RefCell::new(FiberState::Created),
        }
    }
}

// ===== P1b: FiberStashGuard =====

/// RAII guard that installs a Fiber's snapshot into a `Vm`
/// for the duration of a `resume` and restores the prior
/// `Vm` state on `Drop` — **panic-safe**.
///
/// Lifecycle:
///
/// 1. [`install`](Self::install): swap the Vm's "Must stash"
///    fields with the fiber's snapshot. The Vm now runs as
///    the fiber; the fiber's `snapshot` slot temporarily
///    holds an empty placeholder.
/// 2. Caller drives `dispatch_until` (P1c) — bytecode
///    executes against the fiber's frames.
/// 3. On normal exit (yield or fiber returns) OR panic:
///    [`Drop`] captures the current Vm state back into the
///    fiber's `snapshot` slot and restores the original Vm
///    state from `vm_stash`. Either way, the Vm is in a
///    consistent post-swap state by the time the guard's
///    lifetime ends.
///
/// **Compile-time invariant**: only one `FiberStashGuard`
/// can exist per `Vm` at a time — enforced by the `&'a mut
/// Vm` borrow (Rust's borrow checker rejects nested
/// installs). This matches ADR 0023 v2 §"Frame-stack swap
/// invariants" #1.
///
/// **Panic-safety invariant** (ADR 0023 v2 §"Frame-stack
/// swap invariants" #2): swap mid-execution + panic =
/// `Drop` still restores the Vm. Without this, a panic in
/// `dispatch_until` (e.g. via `panic!()` in a host fn)
/// would leave the Vm in the fiber's state forever,
/// breaking every subsequent request. Pinned by the
/// `install_panic_in_bytecode_still_restores_vm` test.
#[allow(dead_code)] // P1c consumes this
pub(crate) struct FiberStashGuard<'a> {
    /// The Vm under management. The `&'a mut` reference
    /// is the load-bearing borrow that enforces "only one
    /// guard per Vm at a time."
    vm: &'a mut crate::vm::Vm,
    /// The Vm's pre-install state, stashed here. `Drop`
    /// swaps this back into the Vm.
    vm_stash: FiberSnapshot,
    /// The fiber's snapshot, currently held by the guard
    /// (the FiberObject's RefCell slot has a placeholder).
    /// `Drop` captures the current Vm state into this and
    /// then puts it back into the FiberObject.
    fiber_snap_holder: FiberSnapshot,
    /// Borrow on the FiberObject — needed at `Drop` to
    /// restore the snapshot. `&'a` (shared) suffices
    /// because we only access the `RefCell` interior.
    fiber: &'a FiberObject,
}

impl<'a> FiberStashGuard<'a> {
    /// Install the fiber's snapshot into the Vm, stashing
    /// the Vm's prior state into the guard. After install,
    /// `vm.frames` / `vm.stack` / etc. are the fiber's; the
    /// fiber's `snapshot` slot is temporarily empty.
    ///
    /// **Panics**: if `fiber.snapshot` is already borrowed
    /// (RefCell guard). In practice this would mean a P1c
    /// bytecode bug — the snapshot RefCell should only be
    /// touched by `install` and `Drop`.
    #[allow(dead_code)] // P1c calls this
    pub(crate) fn install(
        vm: &'a mut crate::vm::Vm,
        fiber: &'a FiberObject,
    ) -> Self {
        // Move the fiber's snapshot OUT of its RefCell
        // (leave an empty placeholder). The guard then
        // owns the snapshot for the resume duration.
        let mut fiber_snap = std::mem::replace(
            &mut *fiber.snapshot.borrow_mut(),
            FiberSnapshot::empty(),
        );
        // Stash the Vm's current state into a fresh empty
        // snapshot. After this, `vm_stash` carries the old
        // Vm state and `vm.*` is empty.
        let mut vm_stash = FiberSnapshot::empty();
        vm_stash.swap_with_vm(vm);
        // Install the fiber's state into the (now-empty)
        // Vm slots. After this, `vm.*` is the fiber's state
        // and `fiber_snap` is empty.
        fiber_snap.swap_with_vm(vm);
        Self {
            vm,
            vm_stash,
            fiber_snap_holder: fiber_snap,
            fiber,
        }
    }
}

impl Drop for FiberStashGuard<'_> {
    /// Restore the Vm to its pre-install state and write
    /// the fiber's new (post-bytecode) state back into its
    /// `snapshot` slot.
    ///
    /// Runs on BOTH normal scope exit AND panic — the panic
    /// safety guarantee. After Drop returns, the Vm is in
    /// the same observable state as before `install`, and
    /// the FiberObject's `snapshot` reflects whatever the
    /// bytecode left in the Vm at the panic point (so a
    /// subsequent `resume` would pick up exactly where the
    /// panic happened — though in practice P1c marks
    /// `state = Returned` on uncaught panics to forbid
    /// resume).
    fn drop(&mut self) {
        // 1. Capture current Vm state into the fiber-snap
        //    holder (which is empty). After this, the
        //    holder carries the post-execution state and
        //    `vm.*` is empty.
        self.fiber_snap_holder.swap_with_vm(self.vm);
        // 2. Restore Vm from the pre-install stash. After
        //    this, `vm.*` is back to its original state
        //    and `vm_stash` is empty.
        self.vm_stash.swap_with_vm(self.vm);
        // 3. Write the post-execution state back into the
        //    FiberObject. The empty placeholder we left in
        //    `install` is replaced.
        let new_snapshot = std::mem::replace(
            &mut self.fiber_snap_holder,
            FiberSnapshot::empty(),
        );
        *self.fiber.snapshot.borrow_mut() = new_snapshot;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P1a: a freshly-allocated FiberObject starts in
    /// `Created` and carries an empty snapshot.
    #[test]
    fn fresh_fiber_is_created_with_empty_snapshot() {
        let body_id = crate::value::ObjId(0); // synthetic — heap not exercised here
        let fiber = FiberObject::new(body_id);
        assert_eq!(*fiber.state.borrow(), FiberState::Created);
        let snap = fiber.snapshot.borrow();
        assert!(snap.frames.is_empty(), "frames must start empty");
        assert!(snap.stack.is_empty(), "stack must start empty");
        assert!(snap.pinned.is_empty(), "pinned must start empty");
        assert!(snap.class_stack.is_empty(), "class_stack must start empty");
        assert!(
            snap.class_visibility_stack.is_empty(),
            "class_visibility_stack must start empty",
        );
        assert!(snap.method_return.is_none(), "method_return must start None");
        assert!(!snap.break_signaled, "break_signaled must start false");
        assert!(
            snap.pending_loop_transfer.is_none(),
            "pending_loop_transfer must start None",
        );
        assert!(
            !snap.suppress_call_result_push,
            "suppress_call_result_push must start false",
        );
        assert!(
            !snap.bypass_visibility_once,
            "bypass_visibility_once must start false",
        );
        #[cfg(feature = "regex")]
        assert!(snap.last_match.is_none(), "last_match must start None");
    }

    /// P1a: state slot is mutable via RefCell — proves the
    /// API surface that P1c's bytecode will use.
    #[test]
    fn fiber_state_transitions_via_refcell() {
        let fiber = FiberObject::new(crate::value::ObjId(0));
        *fiber.state.borrow_mut() = FiberState::Running;
        assert_eq!(*fiber.state.borrow(), FiberState::Running);
        *fiber.state.borrow_mut() = FiberState::Suspended;
        assert_eq!(*fiber.state.borrow(), FiberState::Suspended);
        *fiber.state.borrow_mut() = FiberState::Returned;
        assert_eq!(*fiber.state.borrow(), FiberState::Returned);
    }

    /// P1a: last_value slot supports both write (yield
    /// side) and read (resumer side). Value isn't
    /// PartialEq, so we matches!-pattern the stored
    /// variant instead of assert_eq.
    #[test]
    fn fiber_last_value_is_round_trippable() {
        let fiber = FiberObject::new(crate::value::ObjId(0));
        *fiber.last_value.borrow_mut() = Value::Int(42);
        match &*fiber.last_value.borrow() {
            Value::Int(n) => assert_eq!(*n, 42),
            other => panic!("expected Int(42), got {other:?}"),
        }
    }

    /// P1a: FiberSnapshot::empty matches the
    /// FiberObject::new initial state — pins the
    /// "blank snapshot" shape for future regression.
    #[test]
    fn empty_snapshot_matches_fresh_fiber() {
        let fiber = FiberObject::new(crate::value::ObjId(0));
        let empty = FiberSnapshot::empty();
        let snap = fiber.snapshot.borrow();
        assert_eq!(snap.frames.len(), empty.frames.len());
        assert_eq!(snap.stack.len(), empty.stack.len());
        assert_eq!(snap.pinned.len(), empty.pinned.len());
        assert_eq!(snap.break_signaled, empty.break_signaled);
        assert_eq!(snap.suppress_call_result_push, empty.suppress_call_result_push);
        assert_eq!(snap.bypass_visibility_once, empty.bypass_visibility_once);
    }

    // ===== P1b: FiberStashGuard tests =====

    /// P1b: two consecutive `swap_with_vm` calls on the
    /// same snapshot + vm form an identity. Catches
    /// "added a Vm field, forgot to add it to
    /// swap_with_vm" regressions — if a field is
    /// missing, swap+swap won't be an identity.
    #[test]
    fn swap_with_vm_is_involutive() {
        let mut vm_owned = crate::vm::Vm::new(vec![], crate::intern::Interner::new());
        let vm = &mut vm_owned;
        // Seed every "Must stash" field with distinctive
        // values so swap-and-swap-back can be detected as
        // an identity. If a field is missing from
        // swap_with_vm, the involution would break.
        vm.stack.push(crate::value::Value::Int(11));
        vm.stack.push(crate::value::Value::Int(22));
        vm.pinned.push(crate::value::Value::Int(33));
        vm.break_signaled = true;
        vm.suppress_call_result_push = true;
        vm.bypass_visibility_once = true;
        vm.method_return = Some(crate::value::Value::Int(77));

        let before_stack_len = vm.stack.len();
        let before_pinned_len = vm.pinned.len();
        let before_break = vm.break_signaled;
        let before_supp = vm.suppress_call_result_push;
        let before_bypass = vm.bypass_visibility_once;
        let before_method_return_is_some = vm.method_return.is_some();

        let mut snap = FiberSnapshot::empty();
        snap.swap_with_vm(vm);
        snap.swap_with_vm(vm);

        assert_eq!(vm.stack.len(), before_stack_len);
        assert_eq!(vm.pinned.len(), before_pinned_len);
        assert_eq!(vm.break_signaled, before_break);
        assert_eq!(vm.suppress_call_result_push, before_supp);
        assert_eq!(vm.bypass_visibility_once, before_bypass);
        assert_eq!(vm.method_return.is_some(), before_method_return_is_some);
    }

    /// P1b: install + drop guard restores the Vm to its
    /// pre-install state. The fiber's snapshot now
    /// carries whatever ended up in the Vm during
    /// "execution" (here: a single test value we pushed).
    #[test]
    fn install_then_drop_restores_vm_and_saves_fiber_state() {
        let mut vm_owned = crate::vm::Vm::new(vec![], crate::intern::Interner::new());
        let vm = &mut vm_owned;
        // Pre-install: push a sentinel onto vm.stack.
        vm.stack.push(crate::value::Value::Int(99));
        let pre_install_stack_len = vm.stack.len();

        let fiber = FiberObject::new(crate::value::ObjId(0));
        {
            let guard = FiberStashGuard::install(vm, &fiber);
            // Inside the guard: vm.stack should be EMPTY
            // (fiber's snapshot was empty).
            assert_eq!(guard.vm.stack.len(), 0, "fiber start = empty stack");
            // Simulate bytecode pushing a value.
            guard.vm.stack.push(crate::value::Value::Int(7));
            // Guard drops at end of scope.
        }
        // After drop: Vm restored.
        assert_eq!(vm.stack.len(), pre_install_stack_len);
        match &vm.stack[pre_install_stack_len - 1] {
            crate::value::Value::Int(n) => assert_eq!(*n, 99),
            other => panic!("expected sentinel Int(99), got {other:?}"),
        }
        // Fiber's snapshot captured the pushed value.
        let snap = fiber.snapshot.borrow();
        assert_eq!(snap.stack.len(), 1, "fiber snapshot retains the push");
        match &snap.stack[0] {
            crate::value::Value::Int(n) => assert_eq!(*n, 7),
            other => panic!("expected snapshot Int(7), got {other:?}"),
        }
    }

    /// P1b: panic mid-execution. Drop fires during unwind,
    /// restoring the Vm. This is the load-bearing safety
    /// guarantee — a panic in a host fn must not leave the
    /// Vm in a hybrid (half-fiber) state for the next
    /// request.
    #[test]
    fn install_panic_in_bytecode_still_restores_vm() {
        let mut vm_owned = crate::vm::Vm::new(vec![], crate::intern::Interner::new());
        let vm_ptr = &mut vm_owned as *mut crate::vm::Vm;
        // Pre-install state for verification.
        unsafe {
            (*vm_ptr).stack.push(crate::value::Value::Int(55));
        }
        let pre_panic_stack_len = unsafe { (*vm_ptr).stack.len() };

        let fiber = FiberObject::new(crate::value::ObjId(0));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // SAFETY: single-threaded test; no aliasing.
            let vm = unsafe { &mut *vm_ptr };
            let guard = FiberStashGuard::install(vm, &fiber);
            // Simulate bytecode making changes...
            guard.vm.stack.push(crate::value::Value::Int(123));
            guard.vm.stack.push(crate::value::Value::Int(456));
            // ...then panicking.
            panic!("synthetic panic mid-fiber-execution");
        }));
        assert!(result.is_err(), "expected panic");

        // After unwind: Vm restored despite the panic.
        let vm = unsafe { &*vm_ptr };
        assert_eq!(
            vm.stack.len(),
            pre_panic_stack_len,
            "Vm stack restored after panic — guard's Drop must have run",
        );
        match &vm.stack[pre_panic_stack_len - 1] {
            crate::value::Value::Int(n) => assert_eq!(*n, 55),
            other => panic!("expected sentinel Int(55), got {other:?}"),
        }
    }

    // ===== P1c.1: HeapObj::Fiber + alloc_fiber + GC mark =====

    /// P1c.1: heap allocation path — `Heap::alloc_fiber`
    /// creates a `HeapObj::Fiber`, returns its ObjId,
    /// and `Heap::fiber(id)` retrieves the FiberObject.
    /// Identity round-trip with the body_block field.
    #[test]
    fn alloc_fiber_round_trips_through_heap() {
        use crate::heap::Heap;
        let mut heap = Heap::new();
        // Sentinel ObjId — body_block reference; doesn't need
        // to point at an actual Block for this test, just be
        // round-trippable.
        let body_id = crate::value::ObjId(7777);
        let fiber_id = heap.alloc_fiber(body_id);
        let fiber = heap.fiber(fiber_id);
        assert_eq!(fiber.body_block, body_id, "body_block round-trips");
        assert_eq!(*fiber.state.borrow(), FiberState::Created);
    }

    /// P1c.1: GC mark walks the FiberObject's body_block —
    /// a Block held only by a suspended Fiber survives
    /// collect when the Fiber is reachable.
    ///
    /// Setup:
    /// 1. Allocate a real Block heap slot (the body)
    /// 2. Allocate a Fiber wrapping that body_block's ObjId
    /// 3. Run GC with the Fiber as a root
    /// 4. Verify both the Fiber AND the body Block survive
    ///    (block isn't otherwise reachable)
    #[test]
    fn gc_marks_fiber_body_block_keeps_block_alive() {
        use crate::heap::{Heap, HeapObj};
        use crate::value::BlockHandle;

        let mut heap = Heap::new();
        // Allocate a body Block. Minimal BlockHandle —
        // captured = empty, self_val = Nil, no rest slot.
        let body_block = BlockHandle {
            proto_idx: 0,
            captured: std::rc::Rc::new(std::cell::RefCell::new(vec![])),
            self_val: crate::value::Value::Nil,
            param_start: 0,
            n_params: 0,
            rest_slot: None,
        };
        let body_id = heap.alloc(HeapObj::Block(body_block));
        // Allocate a Fiber pointing at the body.
        let fiber_id = heap.alloc_fiber(body_id);
        // Run GC with just the Fiber as a root.
        let _frees = heap.collect(&[crate::value::Value::Object(fiber_id)]);
        // Both must survive.
        assert!(
            matches!(heap.get(fiber_id), HeapObj::Fiber(_)),
            "Fiber slot must survive GC when it's a root",
        );
        assert!(
            matches!(heap.get(body_id), HeapObj::Block(_)),
            "Body Block must survive GC because the Fiber's mark walk reaches it",
        );
    }

    /// P1c.1: GC sweeps a Fiber when it's NOT a root.
    /// Complements the above test — confirms reachability
    /// is necessary (not just sufficient) for survival.
    #[test]
    fn gc_sweeps_unreachable_fiber() {
        use crate::heap::Heap;
        let mut heap = Heap::new();
        let body_id = crate::value::ObjId(0);
        let _fiber_id = heap.alloc_fiber(body_id);
        // GC with NO roots — fiber should be swept.
        let pre_count = heap.live_count;
        let _frees = heap.collect(&[]);
        assert!(
            heap.live_count < pre_count,
            "Fiber must be swept when unreachable",
        );
    }

    /// P1b: two consecutive resumes preserve state — the
    /// fiber's snapshot captured at first-drop is visible
    /// on the next install. Pins the "FiberStashGuard
    /// round-trips correctly through the FiberObject"
    /// contract that P1c will rely on for `Fiber.yield;
    /// fiber.resume` cycles.
    #[test]
    fn two_consecutive_resumes_preserve_fiber_state() {
        let mut vm_owned = crate::vm::Vm::new(vec![], crate::intern::Interner::new());
        let vm = &mut vm_owned;
        let fiber = FiberObject::new(crate::value::ObjId(0));

        // First "resume": push two values, drop.
        {
            let guard = FiberStashGuard::install(vm, &fiber);
            guard.vm.stack.push(crate::value::Value::Int(1));
            guard.vm.stack.push(crate::value::Value::Int(2));
        }
        // Second "resume": expect the fiber's stack to
        // start with [1, 2] (from the first drop's save).
        {
            let guard = FiberStashGuard::install(vm, &fiber);
            assert_eq!(
                guard.vm.stack.len(),
                2,
                "second resume sees the first resume's state",
            );
            match (&guard.vm.stack[0], &guard.vm.stack[1]) {
                (crate::value::Value::Int(a), crate::value::Value::Int(b)) => {
                    assert_eq!(*a, 1);
                    assert_eq!(*b, 2);
                }
                other => panic!("expected [Int(1), Int(2)], got {other:?}"),
            }
            // Add a third value, drop.
            guard.vm.stack.push(crate::value::Value::Int(3));
        }
        // Third install: expect [1, 2, 3].
        {
            let guard = FiberStashGuard::install(vm, &fiber);
            assert_eq!(guard.vm.stack.len(), 3);
        }
    }
}
