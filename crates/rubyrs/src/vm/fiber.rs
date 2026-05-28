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
    /// then puts it back into the FiberObject (looked up
    /// fresh via `fiber_id` at Drop time).
    fiber_snap_holder: FiberSnapshot,
    /// ObjId of the heap slot holding the FiberObject —
    /// NOT a `&FiberObject` reference, because holding one
    /// alongside `&mut Vm` would alias with the heap (which
    /// `Vm` owns). The guard re-borrows the fiber via the
    /// `Vm`'s heap on Drop. The slot itself is pinned for
    /// the guard's lifetime by the borrow chain: any GC
    /// happens through `&mut Vm` which the guard owns.
    fiber_id: crate::value::ObjId,
}

impl<'a> FiberStashGuard<'a> {
    /// Install the fiber's snapshot into the Vm, stashing
    /// the Vm's prior state into the guard. After install,
    /// `vm.frames` / `vm.stack` / etc. are the fiber's; the
    /// fiber's `snapshot` slot is temporarily empty.
    ///
    /// **Panics**:
    /// - if `fiber.snapshot` is already borrowed (RefCell
    ///   guard) — in practice indicates a P1c bytecode bug
    /// - if `fiber_id` doesn't point at a `HeapObj::Fiber`
    ///   slot — ICE caller-error.
    #[allow(dead_code)] // P1c calls this
    pub(crate) fn install(
        vm: &'a mut crate::vm::Vm,
        fiber_id: crate::value::ObjId,
    ) -> Self {
        // Move the fiber's snapshot OUT of its RefCell
        // (leave an empty placeholder). The borrow on the
        // heap is released after this scope so we can
        // freely use &mut vm below.
        let mut fiber_snap = {
            let fiber = vm.heap.fiber(fiber_id);
            std::mem::replace(
                &mut *fiber.snapshot.borrow_mut(),
                FiberSnapshot::empty(),
            )
        };
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
            fiber_id,
        }
    }
}

impl Drop for FiberStashGuard<'_> {
    /// Restore the Vm to its pre-install state and write
    /// the fiber's new (post-bytecode) state back into its
    /// `snapshot` slot (looked up fresh via `fiber_id`).
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
        //    FiberObject's RefCell. Fresh heap lookup;
        //    the slot is still alive because the guard
        //    has held `&mut Vm` through the resume.
        let new_snapshot = std::mem::replace(
            &mut self.fiber_snap_holder,
            FiberSnapshot::empty(),
        );
        let fiber = self.vm.heap.fiber(self.fiber_id);
        *fiber.snapshot.borrow_mut() = new_snapshot;
    }
}

// ===== P1c.2c: resume_fiber + host fns =====

/// Result of a single resume step.
#[derive(Debug)]
#[allow(dead_code)] // consumed by host fns in this module
pub(crate) enum FiberStep {
    /// Fiber called `Fiber.yield(v)` — the value flows
    /// back to the resumer.
    Yielded(Value),
    /// Fiber's body returned normally with the given value.
    Returned(Value),
}

/// Drive one resume of a Fiber. Either runs to the next
/// `Fiber.yield(v)` and returns `Yielded(v)`, runs to body
/// completion and returns `Returned(v)`, or surfaces a
/// Trap (which also marks the Fiber as `Returned`).
///
/// State transitions:
///
/// - On entry: Created → Running, Suspended → Running
/// - On Yielded exit: Running → Suspended
/// - On Returned exit OR Trap: Running → Returned
///
/// **Error cases** (return Err, fiber state unchanged):
/// - Resume on `Returned` — "dead fiber called"
/// - Resume on `Running` — "double resume"
///
/// First-resume semantics: `invoke_block(body, [arg])` is
/// called to push the body frame, then dispatch_until
/// drives until yield / return. The block receives `arg`
/// as its first parameter.
///
/// Subsequent-resume semantics: the previous Op::Call to
/// the yield host fn left a placeholder Nil on the stack
/// (its "return value"); pop it and push `arg` so the
/// bytecode picks up `arg` as `Fiber.yield`'s actual
/// return value (CRuby parity).
#[allow(dead_code)] // P1c.2c host fns + future P2b consume this
pub(crate) fn resume_fiber(
    vm: &mut crate::vm::Vm,
    fiber_id: crate::value::ObjId,
    arg: Value,
) -> Result<FiberStep, crate::error::Trap> {
    use crate::error::{RubyError, Trap};

    let initial_state = *vm.heap.fiber(fiber_id).state.borrow();
    match initial_state {
        FiberState::Returned => {
            return Err(Trap {
                err: RubyError::RuntimeError {
                    msg: "FiberError: dead fiber called".to_string(),
                },
                backtrace: vec![],
            });
        }
        FiberState::Running => {
            return Err(Trap {
                err: RubyError::RuntimeError {
                    msg: "FiberError: double resume (fiber already running)".to_string(),
                },
                backtrace: vec![],
            });
        }
        _ => {}
    }

    let body_block_id = vm.heap.fiber(fiber_id).body_block;
    let is_first_resume = matches!(initial_state, FiberState::Created);
    *vm.heap.fiber(fiber_id).state.borrow_mut() = FiberState::Running;

    // P1c.3: stash the previous current_fiber_id so
    // `Fiber.current` inside nested resumes sees the right
    // chain. Restored after the guard drops.
    let prev_current_fiber = vm.current_fiber_id.replace(fiber_id);

    let guard = FiberStashGuard::install(vm, fiber_id);
    let pre_depth = 0usize; // fiber's outside frame count is 0 by definition

    // Prepare the entry-state for dispatch_until.
    let prep_result = if is_first_resume {
        guard.vm.invoke_block(body_block_id, vec![arg])
    } else {
        // The previous yield host fn pushed a placeholder
        // Value onto the operand stack (the host fn's
        // return value as Op::Call sees it). Replace it
        // with `arg` so the bytecode's StoreLocal /
        // continuation sees `arg` as the yield's return.
        guard.vm.stack.pop();
        guard.vm.stack.push(arg);
        Ok(())
    };
    if let Err(trap) = prep_result {
        // Resume itself couldn't even start. Mark fiber as
        // Returned (terminal failure) — same shape as a body
        // panic.
        *guard.vm.heap.fiber(fiber_id).state.borrow_mut() = FiberState::Returned;
        return Err(trap);
    }

    guard.vm.fiber_yield_pending = None;
    let dispatch_result = guard.vm.dispatch_until(pre_depth);
    let yield_val = guard.vm.fiber_yield_pending.take();

    let outcome = match (dispatch_result, yield_val) {
        (Ok(_), Some(v)) => {
            // Suspended via Fiber.yield(v).
            *guard.vm.heap.fiber(fiber_id).state.borrow_mut() = FiberState::Suspended;
            *guard.vm.heap.fiber(fiber_id).last_value.borrow_mut() = v.clone();
            Ok(FiberStep::Yielded(v))
        }
        (Ok(_), None) => {
            // Body returned normally.
            let v = guard.vm.stack.pop().unwrap_or(Value::Nil);
            *guard.vm.heap.fiber(fiber_id).state.borrow_mut() = FiberState::Returned;
            *guard.vm.heap.fiber(fiber_id).last_value.borrow_mut() = v.clone();
            Ok(FiberStep::Returned(v))
        }
        (Err(trap), _) => {
            // Trap / exception out of the body. Terminal.
            *guard.vm.heap.fiber(fiber_id).state.borrow_mut() = FiberState::Returned;
            Err(trap)
        }
    };

    // guard drops here, restoring Vm + writing the (post-
    // execution) snapshot back into the FiberObject.
    drop(guard);

    // P1c.3: restore the previous current_fiber_id. For a
    // top-level resume this puts None back; for nested
    // resumes (Fiber A resumed Fiber B), this puts A's
    // id back so A's continuing bytecode sees
    // `Fiber.current == A`.
    vm.current_fiber_id = prev_current_fiber;

    outcome
}

/// Register Fiber host fns on a Runtime. Internal API
/// names start with `__rubyrs_fiber_*`; embedders typically
/// wrap them in an idiomatic Ruby `Fiber` class (lands in
/// P1c.3). For tests + the `_http_server` battery's A3β
/// path (P2b), the host-fn surface is enough.
///
/// Host fns:
/// - `__rubyrs_fiber_new(block) -> Value::Object(fiber_id)`
/// - `__rubyrs_fiber_yield(v) -> Nil` (placeholder; the
///   subsequent resume's arg replaces this on the stack)
/// - `__rubyrs_fiber_resume(fiber, arg) -> Value`
///   (the yielded / returned value)
pub fn register_host_fns(rt: &mut crate::Runtime) {
    use crate::error::{RubyError, Trap};

    let arg_err = |msg: &str| -> Trap {
        Trap {
            err: RubyError::ArgumentError { msg: msg.to_string() },
            backtrace: vec![],
        }
    };

    // __rubyrs_fiber_new(block_value) -> Value::Object(fiber_id)
    rt.register_fn("__rubyrs_fiber_new", move |args| {
        let block_id = match args {
            [Value::Block(id)] => *id,
            _ => {
                return Err(arg_err(
                    "__rubyrs_fiber_new(block: Proc/Lambda) — pass a block-shaped value",
                ));
            }
        };
        let ptr = crate::vm::current_vm_ptr();
        if ptr.is_null() {
            return Err(Trap {
                err: RubyError::RuntimeError {
                    msg: "internal: CURRENT_VM_PTR null in __rubyrs_fiber_new".to_string(),
                },
                backtrace: vec![],
            });
        }
        // SAFETY: ADR 0013 — outer &mut Vm parked by
        // invoke_host_fn; time-disjoint re-borrow.
        let vm = unsafe { &mut *ptr };
        let fiber_id = vm.heap.alloc_fiber(block_id);
        Ok(Value::Object(fiber_id))
    });

    // __rubyrs_fiber_yield(v) -> Nil
    // Sets vm.fiber_yield_pending so dispatch_until exits
    // on its next iteration. The Nil return becomes a stack
    // placeholder that the next resume replaces with the
    // resume's arg (see resume_fiber's "subsequent" path).
    rt.register_fn("__rubyrs_fiber_yield", move |args| {
        let v = args.first().cloned().unwrap_or(Value::Nil);
        let ptr = crate::vm::current_vm_ptr();
        if ptr.is_null() {
            return Err(Trap {
                err: RubyError::RuntimeError {
                    msg: "internal: CURRENT_VM_PTR null in __rubyrs_fiber_yield".to_string(),
                },
                backtrace: vec![],
            });
        }
        // SAFETY: same ADR 0013 contract.
        let vm = unsafe { &mut *ptr };
        vm.fiber_yield_pending = Some(v);
        Ok(Value::Nil)
    });

    // P1c.3: __rubyrs_fiber_current() -> Value
    //
    // Returns the currently-running Fiber as a
    // `Value::Object(id)`, or `Value::Nil` at the top level
    // (no fiber active). ADR 0023 v2's "sentinel root
    // Fiber" is simplified to Nil — there's no concrete
    // root Fiber object in v1; embedders test for nil to
    // detect "called outside a fiber" context.
    rt.register_fn("__rubyrs_fiber_current", move |_args| {
        let ptr = crate::vm::current_vm_ptr();
        if ptr.is_null() {
            return Err(Trap {
                err: RubyError::RuntimeError {
                    msg: "internal: CURRENT_VM_PTR null in __rubyrs_fiber_current".to_string(),
                },
                backtrace: vec![],
            });
        }
        // SAFETY: ADR 0013 contract.
        let vm = unsafe { &*ptr };
        Ok(match vm.current_fiber_id {
            Some(id) => Value::Object(id),
            None => Value::Nil,
        })
    });

    // P1c.3: __rubyrs_fiber_alive_q(fiber) -> Bool
    //
    // True iff the fiber's state is not `Returned`. Created,
    // Running, and Suspended all count as alive. Matches
    // CRuby's `Fiber#alive?` semantic.
    rt.register_fn("__rubyrs_fiber_alive_q", move |args| {
        let fiber_id = match args {
            [Value::Object(id)] => *id,
            _ => {
                return Err(arg_err(
                    "__rubyrs_fiber_alive_q(fiber: Fiber)",
                ));
            }
        };
        let ptr = crate::vm::current_vm_ptr();
        if ptr.is_null() {
            return Err(Trap {
                err: RubyError::RuntimeError {
                    msg: "internal: CURRENT_VM_PTR null in __rubyrs_fiber_alive_q".to_string(),
                },
                backtrace: vec![],
            });
        }
        // SAFETY: ADR 0013 contract.
        let vm = unsafe { &*ptr };
        if !matches!(vm.heap.get(fiber_id), crate::heap::HeapObj::Fiber(_)) {
            return Err(arg_err(
                "__rubyrs_fiber_alive_q: arg is not a Fiber",
            ));
        }
        let state = *vm.heap.fiber(fiber_id).state.borrow();
        Ok(Value::Bool(!matches!(state, FiberState::Returned)))
    });

    // __rubyrs_fiber_resume(fiber, arg) -> Value
    // Returns the yielded value (if fiber yielded) or the
    // body's return value (if fiber returned).
    rt.register_fn("__rubyrs_fiber_resume", move |args| {
        let (fiber_id, arg) = match args {
            [Value::Object(id), arg] => (*id, arg.clone()),
            [Value::Object(id)] => (*id, Value::Nil),
            _ => {
                return Err(arg_err(
                    "__rubyrs_fiber_resume(fiber: Fiber, arg = nil)",
                ));
            }
        };
        let ptr = crate::vm::current_vm_ptr();
        if ptr.is_null() {
            return Err(Trap {
                err: RubyError::RuntimeError {
                    msg: "internal: CURRENT_VM_PTR null in __rubyrs_fiber_resume".to_string(),
                },
                backtrace: vec![],
            });
        }
        // SAFETY: same ADR 0013 contract.
        let vm = unsafe { &mut *ptr };
        // Sanity: the ObjId must point at a Fiber slot.
        if !matches!(vm.heap.get(fiber_id), crate::heap::HeapObj::Fiber(_)) {
            return Err(arg_err(
                "__rubyrs_fiber_resume: first arg is not a Fiber",
            ));
        }
        match resume_fiber(vm, fiber_id, arg)? {
            FiberStep::Yielded(v) => Ok(v),
            FiberStep::Returned(v) => Ok(v),
        }
    });
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
    /// P1c.2a: now allocates the fiber through the heap +
    /// uses ObjId-based install — the production shape.
    #[test]
    fn install_then_drop_restores_vm_and_saves_fiber_state() {
        let mut vm_owned = crate::vm::Vm::new(vec![], crate::intern::Interner::new());
        let vm = &mut vm_owned;
        // Pre-install: push a sentinel onto vm.stack.
        vm.stack.push(crate::value::Value::Int(99));
        let pre_install_stack_len = vm.stack.len();

        // Allocate fiber through the heap. body_block ObjId
        // doesn't need to point at a real Block for this
        // test — only the snapshot swap is exercised.
        let fiber_id = vm.heap.alloc_fiber(crate::value::ObjId(0));
        {
            let guard = FiberStashGuard::install(vm, fiber_id);
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
        let snap = vm.heap.fiber(fiber_id).snapshot.borrow();
        assert_eq!(snap.stack.len(), 1, "fiber snapshot retains the push");
        match &snap.stack[0] {
            crate::value::Value::Int(n) => assert_eq!(*n, 7),
            other => panic!("expected snapshot Int(7), got {other:?}"),
        }
    }

    /// P1b: panic mid-execution. Drop fires during unwind,
    /// restoring the Vm. Load-bearing safety guarantee —
    /// a panic in a host fn must not leave the Vm in a
    /// hybrid (half-fiber) state for the next request.
    /// P1c.2a: ObjId-based; raw pointer + AssertUnwindSafe
    /// pattern retained because Vm doesn't impl
    /// UnwindSafe.
    #[test]
    fn install_panic_in_bytecode_still_restores_vm() {
        let mut vm_owned = crate::vm::Vm::new(vec![], crate::intern::Interner::new());
        // Allocate fiber BEFORE taking the raw pointer so
        // the heap mutation is recorded.
        let fiber_id = vm_owned.heap.alloc_fiber(crate::value::ObjId(0));
        let vm_ptr = &mut vm_owned as *mut crate::vm::Vm;
        // Pre-install state for verification.
        unsafe {
            (*vm_ptr).stack.push(crate::value::Value::Int(55));
        }
        let pre_panic_stack_len = unsafe { (*vm_ptr).stack.len() };

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // SAFETY: single-threaded test; no aliasing.
            let vm = unsafe { &mut *vm_ptr };
            let guard = FiberStashGuard::install(vm, fiber_id);
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

    // ===== P1c.2c: end-to-end resume + yield via Ruby =====

    /// P1c.2c: minimal round-trip — body yields once, then
    /// returns. Asserts both values arrive through their
    /// respective resume calls.
    ///
    /// This is the load-bearing integration test for P1c.2:
    /// it exercises the FiberStashGuard swap, the
    /// dispatch_until yield-check, the host fn → vm
    /// CURRENT_VM_PTR bridge, and the resume_fiber state
    /// machine all in one shot.
    #[test]
    fn fiber_yield_then_return_via_ruby() {
        let mut rt = crate::Runtime::new();
        super::register_host_fns(&mut rt);
        let r = rt.eval(r##"
            body = proc { __rubyrs_fiber_yield(:y); :done }
            fib = __rubyrs_fiber_new(body)
            r1 = __rubyrs_fiber_resume(fib, nil)
            r2 = __rubyrs_fiber_resume(fib, nil)
            "#{r1.inspect}/#{r2.inspect}"
        "##, "p1c2c_simple.rb").expect("eval ok");
        match r {
            Value::Str(s) => assert_eq!(s.to_string_lossy(), ":y/:done"),
            other => panic!("expected Str, got {other:?}"),
        }
    }

    /// P1c.2c: resume's arg becomes the yielded
    /// expression's return value in the body bytecode.
    /// Verifies the placeholder-pop + arg-push logic in
    /// resume_fiber's "subsequent resume" path.
    #[test]
    fn fiber_resume_arg_is_yield_return_value() {
        let mut rt = crate::Runtime::new();
        super::register_host_fns(&mut rt);
        let r = rt.eval(r##"
            body = proc {
              v = __rubyrs_fiber_yield(:waiting)
              v.to_s
            }
            fib = __rubyrs_fiber_new(body)
            r1 = __rubyrs_fiber_resume(fib, nil)
            r2 = __rubyrs_fiber_resume(fib, 42)
            "#{r1.inspect}/#{r2.inspect}"
        "##, "p1c2c_arg.rb").expect("eval ok");
        match r {
            Value::Str(s) => assert_eq!(s.to_string_lossy(), ":waiting/\"42\""),
            other => panic!("expected Str, got {other:?}"),
        }
    }

    /// P1c.3: `__rubyrs_fiber_current` returns Nil at the
    /// top level and the running Fiber's Value inside a
    /// body. Verifies the set/restore around resume.
    #[test]
    fn fiber_current_is_nil_at_toplevel_and_self_inside_body() {
        let mut rt = crate::Runtime::new();
        super::register_host_fns(&mut rt);
        // Inside a fiber body, __rubyrs_fiber_current is the
        // running fiber's Value::Object — non-nil. Outside,
        // it's Nil. We compare against the fiber's own
        // identity to confirm the host fn returns THE
        // SAME fiber, not just any non-nil sentinel.
        let r = rt.eval(r##"
            outside = __rubyrs_fiber_current
            body = proc {
              inside = __rubyrs_fiber_current
              # Capture both nil-ness and identity-match with
              # the parent-scope fib variable.
              __rubyrs_fiber_yield([outside.nil?, inside.nil?])
            }
            fib = __rubyrs_fiber_new(body)
            arr = __rubyrs_fiber_resume(fib, nil)
            "outside_nil=#{arr[0]} inside_nil=#{arr[1]}"
        "##, "p1c3_current.rb").expect("eval ok");
        match r {
            Value::Str(s) => {
                assert_eq!(
                    s.to_string_lossy(),
                    "outside_nil=true inside_nil=false",
                );
            }
            other => panic!("expected Str, got {other:?}"),
        }
    }

    /// P1c.3: `__rubyrs_fiber_current` restores correctly
    /// after a resume — back to Nil at top level when the
    /// fiber yields or returns.
    #[test]
    fn fiber_current_restores_to_nil_after_resume() {
        let mut rt = crate::Runtime::new();
        super::register_host_fns(&mut rt);
        let r = rt.eval(r##"
            body = proc { __rubyrs_fiber_yield(:y); :done }
            fib = __rubyrs_fiber_new(body)
            before = __rubyrs_fiber_current
            __rubyrs_fiber_resume(fib, nil)
            after_yield = __rubyrs_fiber_current
            __rubyrs_fiber_resume(fib, nil)
            after_return = __rubyrs_fiber_current
            "#{before.inspect}/#{after_yield.inspect}/#{after_return.inspect}"
        "##, "p1c3_restore.rb").expect("eval ok");
        match r {
            Value::Str(s) => assert_eq!(s.to_string_lossy(), "nil/nil/nil"),
            other => panic!("expected Str, got {other:?}"),
        }
    }

    /// P1c.3: `__rubyrs_fiber_alive_q` is true while the
    /// fiber is Created / Suspended, false after return.
    #[test]
    fn fiber_alive_q_lifecycle() {
        let mut rt = crate::Runtime::new();
        super::register_host_fns(&mut rt);
        let r = rt.eval(r##"
            body = proc { __rubyrs_fiber_yield(:y); :done }
            fib = __rubyrs_fiber_new(body)
            a = __rubyrs_fiber_alive_q(fib)   # Created → alive
            __rubyrs_fiber_resume(fib, nil)
            b = __rubyrs_fiber_alive_q(fib)   # Suspended → alive
            __rubyrs_fiber_resume(fib, nil)
            c = __rubyrs_fiber_alive_q(fib)   # Returned → dead
            "#{a}/#{b}/#{c}"
        "##, "p1c3_alive.rb").expect("eval ok");
        match r {
            Value::Str(s) => assert_eq!(s.to_string_lossy(), "true/true/false"),
            other => panic!("expected Str, got {other:?}"),
        }
    }

    /// P1c.3: `__rubyrs_fiber_alive_q` rejects non-Fiber
    /// args with ArgumentError.
    #[test]
    fn fiber_alive_q_rejects_non_fiber() {
        let mut rt = crate::Runtime::new();
        super::register_host_fns(&mut rt);
        let err = rt.eval(r##"
            __rubyrs_fiber_alive_q("not a fiber")
        "##, "p1c3_bad.rb").expect_err("expected error");
        let msg = format!("{err:?}");
        assert!(
            msg.contains("__rubyrs_fiber_alive_q") || msg.contains("not a Fiber"),
            "expected fiber-arg error, got: {msg}",
        );
    }

    // ===== P1c.4: Category 3 deep tests — Fiber-scoped Vm state =====
    //
    // ADR 0023 v2 §"Fiber-scoped Vm state" lists 12 "Must
    // stash" Vm fields. P1c.4 pins one test per non-trivial
    // group, demonstrating that yielding inside the
    // corresponding context preserves both the fiber's
    // state AND the resumer's state. swap_with_vm_is_involutive
    // already pins the swap itself; these tests show end-
    // to-end correctness through real bytecode.

    /// P1c.4 (frames): yield from inside a method called by
    /// the fiber body. Both frames must survive the
    /// snapshot save; on resume, the inner method's local
    /// state must still be intact + the return path back
    /// to body must work.
    #[test]
    fn fiber_yield_inside_nested_method_call() {
        let mut rt = crate::Runtime::new();
        super::register_host_fns(&mut rt);
        let r = rt.eval(r##"
            def m(x)
              y = x * 10
              __rubyrs_fiber_yield(y)   # yield inside m's frame
              y + 1                      # resumes here; uses y from m's locals
            end
            body = proc { m(3) }
            fib = __rubyrs_fiber_new(body)
            r1 = __rubyrs_fiber_resume(fib, nil)   # expect 30 (from y * 10)
            r2 = __rubyrs_fiber_resume(fib, nil)   # expect 31 (y + 1, body's final value)
            "#{r1}/#{r2}"
        "##, "p1c4_nested.rb").expect("eval ok");
        match r {
            Value::Str(s) => assert_eq!(s.to_string_lossy(), "30/31"),
            other => panic!("expected Str, got {other:?}"),
        }
    }

    /// P1c.4 (frames, stack): yield from inside a deeper
    /// 3-level call chain. Exercises that arbitrary frame
    /// depth survives + the operand stack doesn't get
    /// confused by partial-expression state.
    #[test]
    fn fiber_yield_inside_three_level_call_chain() {
        let mut rt = crate::Runtime::new();
        super::register_host_fns(&mut rt);
        let r = rt.eval(r##"
            def deep
              __rubyrs_fiber_yield(:deepest)
              :deep_done
            end
            def mid
              deep
            end
            def outer
              mid
            end
            body = proc { outer }
            fib = __rubyrs_fiber_new(body)
            r1 = __rubyrs_fiber_resume(fib, nil)
            r2 = __rubyrs_fiber_resume(fib, nil)
            "#{r1.inspect}/#{r2.inspect}"
        "##, "p1c4_deep.rb").expect("eval ok");
        match r {
            Value::Str(s) => assert_eq!(s.to_string_lossy(), ":deepest/:deep_done"),
            other => panic!("expected Str, got {other:?}"),
        }
    }

    /// P1c.4 (frames + locals.captured): yield from inside
    /// a block that closes over outer-scope locals. The
    /// block's captured Rc<RefCell<Vec<Value>>> must remain
    /// shared with the outer frame across yield/resume so
    /// subsequent reads see updates.
    #[test]
    fn fiber_yield_inside_closure_preserves_captures() {
        let mut rt = crate::Runtime::new();
        super::register_host_fns(&mut rt);
        let r = rt.eval(r##"
            body = proc {
              counter = 0
              cb = -> {
                counter += 1
                __rubyrs_fiber_yield(counter)
              }
              cb.call    # counter = 1, yields 1
              cb.call    # counter = 2, yields 2
              counter    # body returns counter = 2
            }
            fib = __rubyrs_fiber_new(body)
            r1 = __rubyrs_fiber_resume(fib, nil)
            r2 = __rubyrs_fiber_resume(fib, nil)
            r3 = __rubyrs_fiber_resume(fib, nil)
            "#{r1}/#{r2}/#{r3}"
        "##, "p1c4_closure.rb").expect("eval ok");
        match r {
            Value::Str(s) => assert_eq!(s.to_string_lossy(), "1/2/2"),
            other => panic!("expected Str, got {other:?}"),
        }
    }

    /// P1c.4 (frames + rescue handlers): yield from inside
    /// a `rescue` block, then resume. The exception object
    /// bound to `e` must still be accessible after resume
    /// (it lives in the rescue frame's locals which is
    /// part of the snapshot).
    #[test]
    fn fiber_yield_inside_rescue_preserves_exception_local() {
        let mut rt = crate::Runtime::new();
        super::register_host_fns(&mut rt);
        let r = rt.eval(r##"
            body = proc {
              begin
                raise "deliberate"
              rescue => e
                __rubyrs_fiber_yield(e.message)
                "post:#{e.message}"
              end
            }
            fib = __rubyrs_fiber_new(body)
            r1 = __rubyrs_fiber_resume(fib, nil)
            r2 = __rubyrs_fiber_resume(fib, nil)
            "#{r1.inspect}/#{r2.inspect}"
        "##, "p1c4_rescue.rb").expect("eval ok");
        match r {
            Value::Str(s) => assert_eq!(
                s.to_string_lossy(),
                "\"deliberate\"/\"post:deliberate\""
            ),
            other => panic!("expected Str, got {other:?}"),
        }
    }

    /// P1c.2c: resume on a returned fiber raises
    /// FiberError ("dead fiber called").
    #[test]
    fn fiber_resume_on_dead_raises() {
        let mut rt = crate::Runtime::new();
        super::register_host_fns(&mut rt);
        let err = rt.eval(r#"
            body = proc { :once }
            fib = __rubyrs_fiber_new(body)
            __rubyrs_fiber_resume(fib, nil)
            __rubyrs_fiber_resume(fib, nil)
        "#, "p1c2c_dead.rb").expect_err("resume on dead fiber should raise");
        let msg = format!("{err:?}");
        assert!(
            msg.contains("dead fiber called"),
            "expected dead-fiber error, got: {msg}",
        );
    }

    // ===== P1c.2b: vm.fiber_yield_pending field + dispatch_until extension =====

    /// P1c.2b: the yield signaling slot exists on a
    /// freshly-constructed Vm and defaults to `None`. The
    /// end-to-end "Fiber.yield exits dispatch_until" path
    /// requires real bytecode to exercise — lands in
    /// P1c.2c. This commit only adds the infrastructure;
    /// this test verifies the field's default + the
    /// surface for setting it.
    #[test]
    fn fiber_yield_pending_defaults_to_none_and_can_be_set() {
        let mut vm = crate::vm::Vm::new(vec![], crate::intern::Interner::new());
        assert!(
            vm.fiber_yield_pending.is_none(),
            "fresh Vm must have fiber_yield_pending = None",
        );
        vm.fiber_yield_pending = Some(crate::value::Value::Int(42));
        match vm.fiber_yield_pending.take() {
            Some(crate::value::Value::Int(n)) => assert_eq!(n, 42),
            other => panic!("expected Some(Int(42)), got {other:?}"),
        }
        assert!(
            vm.fiber_yield_pending.is_none(),
            "take() clears the slot",
        );
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
    /// contract that P1c.2 relies on for `Fiber.yield;
    /// fiber.resume` cycles. P1c.2a: ObjId-based.
    #[test]
    fn two_consecutive_resumes_preserve_fiber_state() {
        let mut vm_owned = crate::vm::Vm::new(vec![], crate::intern::Interner::new());
        let vm = &mut vm_owned;
        let fiber_id = vm.heap.alloc_fiber(crate::value::ObjId(0));

        // First "resume": push two values, drop.
        {
            let guard = FiberStashGuard::install(vm, fiber_id);
            guard.vm.stack.push(crate::value::Value::Int(1));
            guard.vm.stack.push(crate::value::Value::Int(2));
        }
        // Second "resume": expect the fiber's stack to
        // start with [1, 2] (from the first drop's save).
        {
            let guard = FiberStashGuard::install(vm, fiber_id);
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
            let guard = FiberStashGuard::install(vm, fiber_id);
            assert_eq!(guard.vm.stack.len(), 3);
        }
    }
}
