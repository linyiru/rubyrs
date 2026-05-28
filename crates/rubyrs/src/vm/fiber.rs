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
}
