use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::bytecode::Proto;
use crate::error::Trap;
use crate::heap::Heap;
use crate::intern::{FxHashMap, Interner, SymId};
use crate::value::{Class, FixedArity, Method, ObjId, Value, Visibility};

mod array;
pub(crate) mod bignum;
#[cfg(feature = "_fiber")]
pub(crate) mod fiber;
#[cfg(feature = "cext")]
mod cext;
#[cfg(all(feature = "cext", target_os = "wasi"))]
mod cext_wasi;
mod vm_ptr;
pub(crate) mod dispatch;
mod fileops;
mod gc;
mod hash;
mod iter;
mod kernel;
pub(crate) mod lookup;
#[cfg(feature = "regex")]
mod match_data;
// `pub(crate)` so `jit_tier2`'s `t2_interp_to_s` can reuse the
// shared `integer_to_s_value` (campaign P6b) — same
// no-drift-between-entry-points contract the numeric_call arm and
// do_call's primitive fast path already rely on.
pub(crate) mod numeric;
mod primitive;
mod raise;
mod range;
mod sort;
mod sprintf;
pub(crate) mod step;
pub(crate) mod str2int;
mod string;
mod util;
#[cfg(feature = "bignum")]
pub(crate) use bignum::bigint_equals_float_lossless;
// `with_vm_ptr_set` lives in `vm_ptr` (extracted from cext in
// PoC stage 4a). Re-export from here so both the cext bridge
// and the `_http_server` battery's dispatch path find it via
// `super::with_vm_ptr_set`.
#[cfg(any(
    all(feature = "cext", not(target_os = "wasi")),
    feature = "_http_server",
    feature = "_fiber",
    feature = "_json_native",
    feature = "_yaml_native",
    feature = "_liquid_native",
    feature = "_sqlite",
))]
pub(crate) use vm_ptr::with_vm_ptr_set;
// `current_vm_ptr` is the read-side used by host fn bodies
// that re-enter the Vm. _http_server battery's per-request
// handler uses this to access &mut Vm without the host fn
// signature itself needing to carry one. Stage 4c.3 uses
// this in production code path (handle_request_with_app).
// Also used by `Runtime::reset_between_requests` for a
// cext-invariant debug_assert.
//
// The native-accelerator host fns (_json_native / _yaml_native /
// _liquid_native / _sqlite) read it too, as do the `_prism_native`
// modules (prism_native's materialize path, prism_wq's tokenize
// host fns, commdrv's walk driver) — each is listed so a
// `--no-default-features --features <accel>` build compiles.
// (The prism trio was ALWAYS-compiled until the `_prism_native`
// gate landed, which is why this re-export was briefly
// unconditional.) In configurations where no feature enables the
// V1 dispatch wrap the ptr is never set, so the host fns see null
// and decline / error out of host-fn scope — degraded but correct.
#[cfg(any(
    all(feature = "cext", not(target_os = "wasi")),
    feature = "_http_server",
    feature = "_fiber",
    feature = "_json_native",
    feature = "_yaml_native",
    feature = "_liquid_native",
    feature = "_sqlite",
    feature = "_prism_native",
))]
pub(crate) use vm_ptr::current_vm_ptr;

// `iter::BlockStep` is the result of `step_block`. The
// `_http_server` battery's block-invocation helper
// (stage 4c.2) returns one of these variants. Re-export
// behind the feature gate so http_server.rs can name it
// without reaching into a private mod.
#[cfg(feature = "_http_server")]
pub(crate) use iter::BlockStep;
pub(crate) use numeric::{floor_div_i64, floor_mod_i64, int_cmp_float_lossless};
pub(crate) use lookup::{class_is_a, class_reaches_via_chain, flatten_ancestors, CallCache};
pub use lookup::IcStats;
pub(crate) use primitive::primitive_call;
pub(crate) use sprintf::ruby_sprintf;
pub(crate) use util::{value_cmp_v, value_cmp_v_heap, vec_nil, visibility_from_name};

// ---------- VM ----------



/// Where a frame's local-variable slots live.
///
/// `Shared` is the historical representation — an `Rc<RefCell<Vec>>`
/// cell that blocks capture (`Op::CreateBlock` clones the Rc into the
/// `BlockHandle`), `define_method` closures share, and the writeback /
/// lexical-owner machinery identifies frames by (`Rc::ptr_eq`).
///
/// `Stack(base)` is the escape-analysed fast representation: the
/// method's `n_locals` slots live contiguously in `Vm::locals_arena`
/// starting at `base`. Eligibility is decided at compile time —
/// `Proto::creates_block == false` for a method proto means nothing in
/// the body can ever observe the locals cell (no block capture, and
/// rubyrs has no `Binding`/`local_variables` reflection), so reads and
/// writes skip the Rc deref + RefCell borrow flag entirely and frame
/// push/pop is an arena bump/truncate. Block frames, closures, class
/// bodies, toplevel and eval frames are always `Shared`; every
/// `Rc::ptr_eq`-based identity walk (writeback chain, lexical owner,
/// non-local return target) therefore only ever needs the `Shared` arm
/// — a `Stack` frame can never be a block's lexical owner.
pub(crate) enum Locals {
    Shared(Rc<RefCell<Vec<Value>>>),
    Stack(u32),
}

impl Locals {
    /// The `Shared` cell, or `None` for a `Stack` frame. Identity
    /// walks (`Rc::ptr_eq` against a captured cell) use this so the
    /// `Stack` arm uniformly reads as "not this frame".
    #[inline]
    pub(crate) fn as_shared(&self) -> Option<&Rc<RefCell<Vec<Value>>>> {
        match self {
            Locals::Shared(rc) => Some(rc),
            Locals::Stack(_) => None,
        }
    }
}

/// Env-gated (`RUBYRS_BLOCK_PROF`) phase counters for the block
/// frame-push path (ADR 0037 block-frame residue), in the
/// `RUBYRS_JIT_STATS` family of always-compiled diagnostics: with the
/// env unset every site is a predicted-untaken branch (measured below
/// walk noise). Ticks are cntvct_el0 (24 MHz on Apple Silicon — coarse
/// per read, but unbiased accumulated over ~1M invocations); dumped at
/// Runtime drop by `Vm::dump_block_prof`.
#[derive(Default)]
pub(crate) struct BlockProf {
    /// invocation counts: [ib1, ib2, general, ib1/2→general fallbacks]
    pub(crate) n: [u64; 4],
    /// phase tick totals: [snapshot, gates, argprep, locals, bind, push, reent, recycle]
    pub(crate) t: [u64; 8],
    pub(crate) n_share: u64,
    pub(crate) n_copy: u64,
    pub(crate) copy_slots: u64,
    pub(crate) reent_frames: u64,
    pub(crate) n_recycle: u64,
    /// ib1 fallback reasons: [rest, kw_rest, nparams>1, lambda_arity,
    /// kw_params, block_param_slot]; ib2 reasons fold into the same slots.
    pub(crate) fb: [u64; 6],
    /// general-path shape census: [autosplat_fired, rest_built, kw_any,
    /// blockparam_any, plain_n0, plain_n1]
    pub(crate) gshape: [u64; 6],
}

#[inline(always)]
pub(crate) fn bp_now(on: bool) -> u64 {
    if !on {
        return 0;
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        let t: u64;
        std::arch::asm!("mrs {t}, cntvct_el0", t = out(reg) t, options(nomem, nostack, preserves_flags));
        t
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        0
    }
}

pub(crate) struct Frame {
    pub(crate) proto_idx: usize,
    pub(crate) ip: usize,
    pub(crate) locals: Locals,
    pub(crate) self_val: Value,
    pub(crate) base_sp: usize,
    pub(crate) is_class_body: bool,
    pub(crate) swap_return: Option<Value>,
    /// Block passed to this method, as a heap-managed `Value::Block`
    /// id. Used by `yield`. `None` if the method was called without
    /// a block. Since P2-13 the block lives in the GC heap and we
    /// reference it by `ObjId` — earlier code held an
    /// `Rc<BlockHandle>` here which could cycle.
    pub(crate) block_arg: Option<ObjId>,
    /// Defining class of the method this frame is running.
    /// Used by `Op::Super` — `super` lookup starts at this
    /// class's superclass, not at `self.class.superclass` (the
    /// latter would re-find the current method on a sub-class
    /// instance, causing infinite recursion through the
    /// chain). `None` for blocks, toplevel `<main>`, class
    /// bodies; only methods set this.
    pub(crate) defining_class: Option<Rc<Class>>,
    /// Lexical class for `@@cvar` resolution in a block frame — copied
    /// from the running block's `BlockHandle::lexical_cvar_class`. CRuby
    /// resolves class variables through the lexical cref, not `self`, so
    /// `surrounding_class` prefers this over `self_val` whenever it's
    /// set (it's only set on block frames; method / class-body /
    /// toplevel frames leave it `None` and resolve via `self_val`).
    pub(crate) lexical_cvar_class: Option<Rc<Class>>,
    /// CRuby's `$~` (and the derived `$1`/`$2`/`` $` ``/`$'`/`$&`) is
    /// FRAME-LOCAL to a method — a regex match in a callee must not
    /// clobber the caller's match data. `Some(prev)` on a method frame
    /// holds the caller's `last_match` to restore when this frame
    /// returns (the method body starts with `$~` reset to nil); `None`
    /// on a block / class-body / toplevel frame means "don't touch
    /// `$~` on pop" (a block shares its enclosing method's match data).
    /// Without this, liquid's `Condition.new(Expression.parse($1), $2,
    /// $3)` lost `$2`/`$3` because `Expression.parse` ran a regex that
    /// overwrote the single global match. Gated on `regex` like
    /// `Vm::last_match` / `LastMatch` themselves — the wasm/regex-off
    /// build has no match state to scope.
    #[cfg(feature = "regex")]
    pub(crate) saved_last_match: Option<Option<Box<LastMatch>>>,
    /// True for frames pushed by `Vm::invoke_block` (the frame
    /// for a `do…end` / `{ … }` body). Used by the non-local
    /// `return`-from-block path: when `Op::ReturnMethod` sets
    /// `method_return`, the dispatch loops pop frames while
    /// `is_block` is true, then pop one more frame to exit the
    /// enclosing method. Method frames, class bodies, and the
    /// toplevel `<main>` keep `false`.
    pub(crate) is_block: bool,
    /// True for a block frame whose block is a LAMBDA. A `return`
    /// inside a lambda is LOCAL — it returns from the lambda, not the
    /// lexically-enclosing method (the proc behaviour). The return
    /// unwind treats such a frame as a barrier: `find_return_target`
    /// stops at the nearest enclosing lambda frame, and the unwind
    /// target search accepts it like a method frame. Only set on the
    /// block frames pushed by `invoke_block`/`invoke_block1` for a
    /// lambda; every other frame (method, class body, toplevel,
    /// ordinary block) keeps `false`.
    pub(crate) is_lambda: bool,
    /// Count of positional args the caller supplied. Method
    /// dispatch (`invoke_method_with_block`) sets this to
    /// `positional_take`; `Op::JumpIfArgGiven(slot, off)` consults
    /// it to decide whether `slot` was caller-supplied or left
    /// for the default-arg prologue to fill. Block / class-body
    /// / toplevel frames all use 0 — they don't carry an arity
    /// model that the prologue op would consult.
    pub(crate) n_given_positional: u16,
    /// Bitmap of caller-supplied keyword args, indexed by the
    /// method's 0-based kwarg position (NOT local slot index).
    /// `Op::JumpIfKwArgGiven(kw_idx, off)` consults bit
    /// `1 << kw_idx`. Set by the kwarg binder when the caller
    /// supplied a value for that name. Block / class-body /
    /// toplevel frames use 0 (they don't model kwargs the
    /// prologue would consult). 64-bit caps non-literal-default
    /// kwargs per method at 64 — far above any real signature.
    pub(crate) kw_given_mask: u64,
    /// Exception / loop bookkeeping, boxed out of the hot struct.
    /// Most frames (every block invocation in a tight iterator
    /// loop, most method calls) never enter a `begin`, install a
    /// rescue, or run a `while` — for them this stays `None`: no
    /// allocation, and `Frame`'s push/pop memmove shrinks by the
    /// four inline `Vec` headers (328 → 240 bytes measured). The
    /// box is created lazily by `aux_mut()` at the first
    /// EnterBegin / PushRescue / EnterLoop. GC: `gc.rs`'s root
    /// gather walks `begin_rescue_depths[].saved_dollar_bang`
    /// through here (the `e0c664ab` rooting fix).
    pub(crate) aux: Option<Box<FrameAux>>,
    /// ADR 0024 Phase A: `Op::Yield` synchronous-wrapper
    /// "yield in progress" flag. Set by `Op::Yield`'s match
    /// arm BEFORE `invoke_block`, cleared after the nested
    /// `dispatch_until` returns normally. On Fiber yield
    /// mid-block, the flag stays set and is stashed in
    /// FiberSnapshot (ADR 0023 v7 §"Fiber-scoped Vm state");
    /// resume reads it and SKIPS the invoke_block step
    /// (block frame already on the stack), going straight to
    /// the post-block branch. Per-Frame so nested concurrent
    /// yields each track their own pending state.
    #[allow(dead_code)] // wired in Phase A.1
    pub(crate) pending_yield: bool,
    /// Set on block frames whose `locals` is a FRESH per-invocation
    /// Vec (the copy path of `block_frame_locals`). Holds the
    /// original `captured` Rc + the block's `param_start`. Since the
    /// outer-chain routing model landed, NO value copy-back happens
    /// through this — outer-slot reads/writes route directly to the
    /// canonical binding cell (`outer_cell_for`). The field's
    /// remaining role is Rc-IDENTITY for the lexical walks
    /// (`find_lexical_owner_frame` / `find_return_target` /
    /// `Op::ReturnMethod`'s owner stash): each block frame's
    /// writeback points one scope outward so `yield` / non-local
    /// `return` / `super` locate the lexical owner method.
    /// `None` for method / class-body / toplevel frames and
    /// share-direct block frames (their `locals` IS the outer cell,
    /// so the identity walk already matches).
    pub(crate) block_writeback: Option<(Rc<RefCell<Vec<Value>>>, u16)>,
    /// First slot this frame's own locals cell canonically OWNS.
    /// `0` for method / class-body / toplevel frames and for
    /// share-direct block frames (the cell IS the outer scope);
    /// the block's `param_start` for copy-path block frames and
    /// `define_method`-closure frames. Slot accesses BELOW this
    /// boundary are captured outer locals — they route via
    /// `outer_cell_for` to the canonical binding cell, so every
    /// closure capturing a variable (and its defining scope)
    /// reads/writes the SAME slot, even after intermediate
    /// frames pop (CRuby shared-binding semantics).
    pub(crate) own_start: u16,
    /// `true` on a define_method SHARE-DIRECT frame (`Locals` is the
    /// closure's captured cell itself — see the dm arm of the method
    /// dispatch). Every frame-pop site decrements
    /// `Vm::dm_share_depth` when set, so the dm dispatch can gate
    /// its share fast path on `dm_share_depth == 0` (an O(1) check)
    /// instead of scanning the frame stack for re-entrancy.
    pub(crate) dm_share: bool,
    /// Boundary of the `outer_cell` region: routed slots `>=
    /// outer_cell_start` live in `outer_cell` (the running handle's
    /// `captured` — the CREATING scope's cell); routed slots below it
    /// live in `outer_rest`. Copied from `BlockHandle::creator_start`
    /// / `MethodClosure::creator_start`; 0 for non-routing frames.
    pub(crate) outer_cell_start: u16,
    /// The creating scope's canonical cell for routed slots
    /// `[outer_cell_start, own_start)` — the running handle's
    /// `captured` Rc. `None` whenever `own_start == 0` (nothing to
    /// route). Read by `Op::CreateBlock` to derive deeper closures'
    /// chains and by the GC root walk (the ORIGINAL binding cell may
    /// be reachable only through here while this frame runs).
    pub(crate) outer_cell: Option<Rc<RefCell<Vec<Value>>>>,
    /// Ancestor canonical-owner chain for routed slots
    /// `< outer_cell_start` — cloned from the running handle's
    /// `outer_chain`. `None` for depth-1 closures (creator owns all
    /// captured slots). GC-walked like `outer_cell`.
    pub(crate) outer_rest: Option<crate::value::OuterChain>,
    /// Set on block frames from the running `BlockHandle`'s
    /// `captured_yield_block` (the block belonging to the method that
    /// lexically encloses this block). `Op::Yield` reads it as the
    /// fallback when the lexical owner method is no longer on the stack
    /// — the escaped-closure case where `lexical_owner_of_top` finds no
    /// live method frame. `None` on method / class-body / toplevel
    /// frames and on blocks whose enclosing method had no block.
    pub(crate) captured_yield_block: Option<ObjId>,
}

/// The cold half of `Frame` — exception-handling and `while`-loop
/// bookkeeping, lazily boxed (see `Frame::aux`). Field semantics are
/// unchanged from when these lived inline on `Frame`:
///
/// - `rescues`: active handlers, pushed by `Op::PushRescue` /
///   `Op::PushEnsure`, consumed by the unwinder.
/// - `loop_rescue_depths`: one `rescues.len()` snapshot per enclosing
///   active `while` (`Op::EnterLoop` pushes, `Op::ExitLoop` pops;
///   `Op::BreakLoop` reads the top to discard handler entries before
///   jumping to the loop end).
/// - `loop_stack_depths`: parallel stack of `stack.len()` snapshots
///   at each `Op::EnterLoop`, used by `continue_loop_transfer` to
///   truncate operand-stack residue (kept in lock-step with
///   `loop_rescue_depths`: same push / pop / truncate sites).
/// - `begin_rescue_depths`: per-`begin` baselines for `retry`'s
///   rescue-stack truncation and the `$!` snapshot restore
///   (code-review #306 round 1 / the `3134717c` dynamic scoping).
#[derive(Default)]
pub(crate) struct FrameAux {
    /// The RUNTIME name a `define_method`-installed method was
    /// invoked under. `def`-compiled methods bake their name into
    /// the proto, but a define_method body is a BLOCK proto whose
    /// compile-time name is its lexical context — useless for
    /// `super()` resolution and `__method__` (minitest/spec's
    /// before/after hooks are `define_method :setup do super() …`).
    /// The closure-method invoke path stamps the installed name
    /// here; `super_runtime_name` and `__method__` prefer it.
    /// Lives in the cold aux box so plain frames don't grow.
    pub(crate) invoked_name: Option<crate::intern::SymId>,
    /// Set on an `instance_eval` / `instance_exec` block frame to its
    /// receiver. A bare `def name; end` inside such a block defines a
    /// SINGLETON method on this receiver (CRuby), not a toplevel method —
    /// `Op::Def` consults this before the class-stack / toplevel fallback.
    /// `None` on every other frame (lives in the cold aux box).
    pub(crate) instance_eval_definee: Option<Value>,
    pub(crate) rescues: Vec<RescueHandler>,
    pub(crate) loop_rescue_depths: Vec<usize>,
    pub(crate) loop_stack_depths: Vec<usize>,
    pub(crate) begin_rescue_depths: Vec<BeginBaseline>,
}

/// Write `slot` into a routed canonical binding cell — the store half
/// of the capture-routing pair (see `Frame::outer_cell_for`). Replaces
/// the old `propagate_outer_write` frame-stack walk: routing hits the
/// ORIGINAL binding cell directly, so it works even when the defining
/// frames are no longer on the stack (escaped procs, deferred Thread
/// bodies, suspended Fibers).
#[inline]
pub(crate) fn cell_store(cell: &Rc<RefCell<Vec<Value>>>, slot: usize, v: Value) {
    let mut t = cell.borrow_mut();
    if t.len() <= slot {
        // Defensive: scope cells are sized to their proto's n_locals,
        // which covers every capturable slot — but a grow beats
        // silently dropping a user's write.
        t.resize(slot + 1, Value::Nil);
    }
    t[slot] = v;
}

impl Frame {
    /// Capture routing: the CANONICAL binding cell for `slot`, or
    /// `None` when the slot belongs to this frame's own cell. All
    /// slot reads/writes on `Shared` frames consult this first —
    /// `own_start == 0` (method / class-body / toplevel /
    /// share-direct frames) short-circuits on the first compare.
    /// Routing to the original binding (instead of the frame's
    /// per-invocation snapshot) is what makes a captured local ONE
    /// shared binding across the defining scope and every closure,
    /// even after intermediate frames pop.
    #[inline]
    pub(crate) fn outer_cell_for(&self, slot: usize) -> Option<&Rc<RefCell<Vec<Value>>>> {
        if slot >= self.own_start as usize {
            return None;
        }
        if slot >= self.outer_cell_start as usize {
            // Creating scope's region — `outer_cell` is Some whenever
            // own_start > 0; fall through defensively if not.
            if let Some(cell) = &self.outer_cell {
                return Some(cell);
            }
        }
        self.outer_rest
            .as_ref()
            .map(|chain| crate::value::chain_owner_cell(chain, slot))
    }

    /// Get-or-create the aux box. Call sites that only READ should
    /// prefer `aux.as_ref()` / the `..._len` style probes so an
    /// aux-less frame stays allocation-free.
    #[inline]
    pub(crate) fn aux_mut(&mut self) -> &mut FrameAux {
        self.aux.get_or_insert_with(Default::default)
    }
    #[inline]
    pub(crate) fn rescues_len(&self) -> usize {
        self.aux.as_ref().map_or(0, |a| a.rescues.len())
    }
    #[inline]
    pub(crate) fn pop_rescue(&mut self) -> Option<RescueHandler> {
        self.aux.as_mut().and_then(|a| a.rescues.pop())
    }
    #[inline]
    pub(crate) fn loop_depth(&self) -> usize {
        self.aux.as_ref().map_or(0, |a| a.loop_rescue_depths.len())
    }
    #[inline]
    pub(crate) fn begin_depth(&self) -> usize {
        self.aux.as_ref().map_or(0, |a| a.begin_rescue_depths.len())
    }
}

/// In-flight structured `break`/`next` walking through an
/// `ensure` chain. The `kind` carries the break value (or `Next`
/// for `next`); `target_ip` is the instruction the transfer
/// lands at once every intervening `is_ensure` handler has run;
/// `target_loop_depth` is the `loop_rescue_depths` length the
/// frame should have after the transfer (entries pushed by
/// `EnterLoop`s the transfer is escaping out of get truncated).
/// Transfers live on a VM-level STACK (`pending_loop_transfers`):
/// an ensure body run by a suspended transfer can itself contain a
/// `while … break` through another ensure, which suspends a second,
/// inner transfer that must complete before the outer one resumes
/// (`while; begin; break; ensure; while; begin; break; ensure …`).
pub(crate) struct LoopTransfer {
    pub(crate) kind: LoopTransferKind,
    pub(crate) target_ip: usize,
    pub(crate) target_rescues_len: usize,
    pub(crate) target_loop_depth: usize,
    /// `stack.len()` at the time `Op::EnterLoop` ran for this
    /// transfer's target loop. On landing the stack is truncated
    /// to this depth before the break value (if any) is pushed —
    /// flushes any operand-stack residue the body accumulated,
    /// including the exception that `unwind_with_exception` pushed
    /// when it entered an ensure handler we're now `break`ing out
    /// of. Without this, `while; begin; raise; ensure; break; end;
    /// end` leaks the exception value on the operand stack until
    /// the surrounding frame pops.
    pub(crate) target_stack_depth: usize,
    /// `Some` while the transfer's walk is parked inside an
    /// `is_ensure` handler body (set at the suspension point in
    /// `continue_loop_transfer`, cleared when `Op::EndEnsure`
    /// resumes the walk). See [`SuspendCoord`] for how the
    /// coordinates identify "the body's tail reached EndEnsure"
    /// and drive escape-cancellation.
    pub(crate) suspended: Option<SuspendCoord>,
}

/// Where an ensure-walk (loop transfer / method break) is parked.
///
/// Captured at the moment `continue_loop_transfer` /
/// `continue_method_break` jumps into an `is_ensure` handler body.
/// Serves two purposes:
///
/// 1. **EndEnsure identification.** `Op::EndEnsure` at an ensure
///    body's tail resumes a suspended walk only when the walk's
///    coordinates match the CURRENT tail position: same frame
///    index, same `rescues_len` (bodies are push/pop-balanced),
///    same operand-stack depth (bodies are compile_stmt-balanced;
///    an exception-path entry into some ensure sits at depth+1
///    because the unwinder pushed the exception value, so the two
///    entry modes can never alias). `seq` breaks the tie when a
///    nested walk suspends at coordinates identical to its outer
///    walk (e.g. a `while … break`-with-ensure as the FIRST
///    statement of an outer suspended ensure body): the highest
///    seq is the innermost body, and its tail is reached first.
///
/// 2. **Escape cancellation.** A suspended walk stays alive while
///    control remains INSIDE its ensure body (a raise that is
///    rescued within the body — or within a callee — must not
///    cancel a pending `return`/`break`; CRuby resumes it). It is
///    cancelled exactly when control leaves the body without
///    reaching its tail: an exception unwinding to a handler BELOW
///    the suspension baseline, the suspended frame being popped, or
///    a superseding `return`/`break`/`next` crossing out of the
///    body. See `Vm::cancel_transfers_*` and the sweep sites in
///    `unwind_with_exception` / `begin_method_break` /
///    `begin_loop_transfer` / `Op::Return`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SuspendCoord {
    /// Absolute index of the frame whose ensure body we're parked in.
    pub(crate) frame_idx: usize,
    /// `frame.rescues_len()` right after the ensure handler (and
    /// everything above it) was popped — the baseline the body's
    /// tail returns to.
    pub(crate) rescues_len: usize,
    /// `stack.len()` at handler entry (the handler's recorded
    /// `stack_depth`, which the suspension truncated to). The
    /// body is statement-balanced, so its tail sits at exactly
    /// this depth; an exception-path entry sits one higher.
    pub(crate) stack_len: usize,
    /// `frame.loop_depth()` at suspension — the ensure body's
    /// "region" discriminator for `begin_loop_transfer`'s supersede
    /// sweep. A `while`/`until` entered INSIDE the suspended body
    /// pushes `loop_rescue_depths` entries above this mark, so a
    /// break whose target-loop index is `>= loop_depth` is contained
    /// in the body (normal transfer, walk resumes at the body's
    /// tail), while an index `< loop_depth` targets a loop the body
    /// is lexically inside — the break is jumping OUT of the body
    /// and the walk must be cancelled (a stale suspended walk here
    /// would later be mis-replayed by `Op::Return`'s abandoned-walk
    /// branch). Time-based rather than ip-range-based region
    /// modeling: it needs no compiler-layout knowledge and is
    /// immune to the second-copy placement of ensure bodies.
    pub(crate) loop_depth: usize,
    /// Monotonic suspension counter (`Vm::suspend_seq`) — total
    /// order of suspensions for innermost-first resume.
    pub(crate) seq: u64,
}

pub(crate) enum LoopTransferKind {
    Break { value: Value },
    Next,
}

/// ADR 0024 Phase A.4: in-flight block-break unwinding the yielding
/// method through its `ensure` chain. Distinct from `LoopTransfer`
/// because the target is a frame pop + return-value push (single
/// method frame), not a loop join in the same frame.
///
/// Set by `Op::Yield`'s case (a) when the block did
/// `break val` and the yielding method's frame still has pending
/// `is_ensure: true` rescue handlers. Walks them top-down running
/// each ensure body; `Op::EndEnsure` observes this slot (alongside
/// `pending_loop_transfer`) and resumes the walk. Once all ensures
/// have run, lands by popping the yielding method's frame and
/// pushing `value` as its return.
///
/// Phase A.5 extends this to multi-frame walks (yielding method
/// is deeper than the immediate top frame; e.g.
/// `def each; 3.times { yield }; rest...; end` where times's
/// step_block-driven Rust loop sits between yield and each, so
/// after times returns we need to skip "rest..." and run each's
/// ensures before landing). `target_frame_idx` is the absolute
/// frame index the break should land on (the yielding method's
/// frame). Frames above it get popped + have their own ensures
/// run on the way down.
pub(crate) struct MethodBreak {
    pub(crate) value: Value,
    pub(crate) target_frame_idx: usize,
    /// `Some` while control is parked inside an `is_ensure` body
    /// because `continue_method_break` jumped to its handler IP.
    /// The dispatch loops' top-of-iteration check honours this:
    /// they skip firing `continue_method_break` while suspended
    /// so the ensure body runs to completion. `Op::EndEnsure`
    /// clears the slot before re-entering
    /// `continue_method_break`, resuming the walk. The recorded
    /// [`SuspendCoord`] identifies the suspension for EndEnsure
    /// matching and escape cancellation — see its doc.
    pub(crate) suspended: Option<SuspendCoord>,
    /// What started this walk — see [`WalkOrigin`]. Drives the one
    /// CRuby corner behaviour that depends on HOW the walk began
    /// (the `next`-abandonment replay in `Op::Return`); everything
    /// else ignores it.
    pub(crate) origin: WalkOrigin,
}

/// How a [`MethodBreak`] walk began. A `break`/`next` in an ensure
/// body the walk crosses lands at the loop join and CANCELS the walk
/// regardless of origin — CRuby >= 3.4.2 / parse.y / 3.3.x agree
/// (`break` compiles to `throw TAG_BREAK` in the exceptional ensure
/// copy). That's rubyrs's structural default; no origin consulted.
/// (CRuby 3.4.0/3.4.1's Prism compiler diverged for syntactically-
/// local returns — the [Bug #21001] window, fixed upstream by
/// ruby/ruby 31905d9e and backported into 3.4.2. rubyrs's probe
/// matrix predates the fix and mimicked it via a `LocalMethodReturn`
/// variant + an `Op::BreakLoop` artifact branch until S1 removed
/// both; see SUBSET.md "break/next inside a suspended ensure walk".)
///
/// The origin that still matters: a BLOCK-frame `next` abandoning a
/// suspended walk goes the other way — the walk wins, not the `next`
/// (see `Op::Return`'s abandoned-walk branch and the doc on each
/// variant below).
pub(crate) enum WalkOrigin {
    /// Walk begun by consuming the `method_return` signal (a
    /// non-local `return` crossing frames). Carries the lexical
    /// owner's locals Rc (the same identity `method_return_locals`
    /// held) so a block-frame `next` that abandons the suspended
    /// body can convert the walk BACK to the `method_return`
    /// signal: `[..].each { begin return v ensure next end }`
    /// returns v from the enclosing method — a DELIBERATE
    /// divergence (modern CRuby discards the pending walk, which
    /// hangs forever on the bytecode-yielder K4 shape; see
    /// tests/embed/ensure_walk_divergences.rs). Replaying the
    /// signal re-uses the whole established crossing protocol
    /// (dispatch levels, iter drivers) instead of teaching every
    /// driver about in-flight walks. See `Op::Return`'s
    /// abandoned-walk branch.
    MethodReturnSignal(Rc<RefCell<Vec<Value>>>),
    /// Everything else: block-break landing on the yielding method,
    /// local method return / block value-return with ensures,
    /// fiber-recovery breaks.
    Block,
}

/// ADR 0031 increment 2 — precomputed argument-binding plan for a
/// NON-fixed-arity method proto (optional positionals / `*rest` /
/// post-required / `&blk` / all-OPTIONAL keywords — literal OR
/// computed defaults; REQUIRED kwargs and `**kwrest` are INELIGIBLE
/// — see `Vm::nfa_plan_for`). The variadic sibling of `FixedArity`: every
/// field the general binder re-derives from the Proto per call
/// (`invoke_method_with_block_inner`'s tail-layout arithmetic) is
/// captured once here, so the dispatch fast paths can bind a
/// resolved call stack-direct without touching the ~320-byte-stride
/// `protos[idx]` row. Optional-param DEFAULTS need no plan entry:
/// they are compiled as a body-entry prologue (`JumpIfArgGiven` +
/// default expr + `StoreLocal`, keyed on the frame's
/// `n_given_positional`), so the binder's only default job is
/// leaving unfilled slots Nil and stamping the given-count —
/// evaluation order/scope/once-per-call semantics ride on the same
/// bytecode the general binder relies on. Keyword defaults are
/// served only on bare-`Op::Call`-family sites passing zero kwargs
/// (`kw_given_mask = 0`; every kwargs-carrying route declines to
/// the general binder's peel + per-name bind): a LITERAL default
/// (campaign P5a) is the one family the binder itself fills (no
/// prologue exists for it) — the serve clones each literal FRESH
/// from the proto row (see `Vm::kw_literal_default_fresh` for the
/// mutation/frozen contract) — while a COMPUTED default (campaign
/// P6b) leaves its slot Nil for the mask-0 body prologue
/// (`Op::JumpIfKwArgGiven`) to evaluate, exactly as the binder does
/// for a zero-kwargs call.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NfaPlan {
    /// `proto.params.len()` — cross-checked against `m.params.len()`
    /// at the invoke site so an exotic Method whose params diverge
    /// from its proto (none exist today outside closures/builtins,
    /// which the callers already decline) falls back to the cascade.
    pub(crate) params_len: u16,
    /// Leading required positionals (`proto.n_required_positional`).
    pub(crate) required_pre: u16,
    /// Trailing required positionals (`proto.n_required_post`) —
    /// bound from the arg tail BEFORE optionals/rest gather.
    pub(crate) required_post: u16,
    /// pre + optionals + post — the positional slot region
    /// `[0, positional_max)`; the rest slot (when `has_rest`) is AT
    /// `positional_max`, mirroring the general binder's layout.
    pub(crate) positional_max: u16,
    pub(crate) has_rest: bool,
    /// Number of keyword params — non-zero ONLY when every one is
    /// OPTIONAL (a `Some` literal snapshot OR a `None` computed
    /// default with a body prologue; no REQUIRED kwarg, no
    /// `**kwrest`). Their slots sit at
    /// `[positional_max + has_rest, .. + kw_count)`, mirroring the
    /// general binder's tail layout; the serve fills each literal
    /// slot from the proto snapshot (fresh per call) and leaves each
    /// computed slot Nil for the mask-0 prologue, all with
    /// `kw_given_mask = 0`.
    pub(crate) kw_count: u16,
    /// `&blk` param present: its slot is
    /// `positional_max + has_rest + kw_count` (after the kw region,
    /// mirroring the general binder's layout).
    pub(crate) has_block_param: bool,
    pub(crate) n_locals: u16,
    /// Cached `!proto.creates_block` — same contract as
    /// `FixedArity::stack_eligible`.
    pub(crate) stack_eligible: bool,
}

/// Lazy tri-state slot for `Vm::nfa_plans` (index = `proto_idx`).
#[derive(Debug, Clone, Copy)]
pub(crate) enum NfaPlanSlot {
    Unknown,
    Ineligible,
    Plan(NfaPlan),
}

/// Body-shape plan for the "rest-predicate" frame-free fast path
/// (the rubocop-ast `Node#type?(*types)` family — the hottest
/// polymorphic call in a RuboCop cop walk). A pure-rest method whose
/// compiled body is EXACTLY one of two op templates:
///
///   simple:  `def m?(*rest); rest.include?(g); end`
///   grouped: `def m?(*rest)
///               return true if rest.include?(g)
///               tmp = CONST[g]
///               !tmp.nil? && rest.include?(tmp)
///             end`
///
/// where `g` is a bare (implicit-self) zero-arg call, is served
/// WITHOUT a frame, rest-Array materialization, or any body dispatch:
/// the serve resolves `g` through the body's own inline-cache slot
/// (so per-receiver-class overrides and `method_gen` invalidation ride
/// the existing IC), requires the resolution to be a trivial
/// attr_reader (`getter_ivar`), and unrolls the `include?` scans into
/// Symbol identity compares over the caller's still-on-stack args.
/// Exactness is guaranteed by runtime deopts (any non-Symbol arg /
/// ivar / group value falls through to the general path before any
/// observable effect) plus the `method_gen`-revalidated
/// `rest_pred_deps_ok` flag (no user overrides on the builtin methods
/// the body would dispatch: `Array#include?`, `Hash#[]`,
/// `Symbol#==`/`nil?`, `NilClass#nil?`, `TrueClass`/`FalseClass#!`).
#[derive(Debug, Clone, Copy)]
pub(crate) struct RestPredPlan {
    /// The bare zero-arg call name (`type` in rubocop-ast).
    pub(crate) getter_name: crate::intern::SymId,
    /// The body's OWN call-site cache id for that call — reusing it
    /// gives per-receiver-class polymorphic resolution + method_gen
    /// invalidation identical to actually running the body op.
    pub(crate) getter_cache: u32,
    /// How the grouped variant's body loads the fallback-group Hash
    /// constant. `None` = simple variant (no group phase).
    pub(crate) group: RestPredGroup,
}

/// Const-load shape for `RestPredPlan::group`. Serve-time resolution
/// goes through the interpreter's OWN const caches (`const_cache_chain`
/// keyed by (callee proto, chain idx) / `const_cache_flat`), so a
/// cold cache simply declines to the general path — whose body op
/// then resolves + fills the cache for every later serve. `const_gen`
/// invalidation is therefore inherited, never reimplemented.
#[derive(Debug, Clone, Copy)]
pub(crate) enum RestPredGroup {
    /// Simple variant — no group fallback phase.
    None,
    /// Body op 10 is `LoadConstChain(idx)` (bare const read inside a
    /// class/module scope — the rubocop-ast shape).
    Chain(u32),
    /// Body op 10 is `LoadConst(sym)` (toplevel-compiled sibling).
    Flat(crate::intern::SymId),
}

/// Lazy tri-state slot for `Vm::rest_preds` (index = `proto_idx`).
/// A Proto's code is immutable after compile, so `No` / `Pred` are
/// final (redefinition installs a different proto_idx).
#[derive(Debug, Clone, Copy)]
pub(crate) enum RestPredSlot {
    Unknown,
    No,
    Pred(RestPredPlan),
}

/// RAII guard for `Vm.pinned`. Native-side code that needs heap
/// values to survive an intervening `maybe_gc` / `?` early-return
/// constructs one of these, calls `.pin(v)` for every value it
/// wants kept alive, and accesses the VM through `g.vm.foo()` while
/// the guard is in scope. When the guard drops — including on the
/// `?` unwind path — it pops exactly the values it pinned, leaving
/// `pinned` at the same length it had on entry.
///
/// Why this exists: before P0-2, every iterator driver (Array#each,
/// Hash#to_a, the Enumerable filtering family, the
/// `Class.new(args)` allocator) did `self.pinned.push(...); ...; ?;
/// ...; self.pinned.pop();` by hand. The `?` operator could short-
/// circuit past the pop on a raise from a host fn or a fuel trap,
/// leaving dead values on `pinned` that the GC then kept marking as
/// live — slow leak, hard to spot. With this guard the pop is
/// unconditional.
pub(crate) struct PinGuard<'a> {
    pub(crate) vm: &'a mut Vm,
    count: usize,
}

impl<'a> PinGuard<'a> {
    pub(crate) fn new(vm: &'a mut Vm) -> Self {
        Self { vm, count: 0 }
    }
    pub(crate) fn pin(&mut self, v: Value) {
        self.vm.pinned.push(v);
        self.count += 1;
    }
}

impl Drop for PinGuard<'_> {
    fn drop(&mut self) {
        for _ in 0..self.count { self.vm.pinned.pop(); }
    }
}

/// Triple of frame-stack snapshots taken at `Op::EnterBegin`
/// time: `rescues.len()`, `loop_rescue_depths.len()`, and
/// `loop_stack_depths.len()`. On `Op::TruncateRescuesToBeginBaseline`
/// (retry) all three are truncated to these values so a `retry`
/// inside a `while` loop in a rescue body doesn't leave the
/// loop's `EnterLoop` entries leaked into the next iteration of
/// the begin body. (Code-review #306 round 2 — closes the
/// nested-loop-in-rescue gap.)
#[derive(Clone)]
pub(crate) struct BeginBaseline {
    pub(crate) rescues_len: usize,
    pub(crate) loop_rescue_depths_len: usize,
    pub(crate) loop_stack_depths_len: usize,
    /// Value of `$!` (errinfo) captured at this begin region's
    /// `Op::EnterBegin`. CRuby's `$!` is dynamically scoped: it holds
    /// the in-flight exception only WHILE a rescue/ensure body runs,
    /// then reverts when the begin region is left. `Op::ExitBegin`
    /// restores `$!` to this snapshot; so does `Op::Return` out of a
    /// rescue body (which skips `ExitBegin`). Without it, a handled
    /// exception leaks into `$!` and a later bare `raise` re-raises the
    /// stale exception. (ADR 0025 deferred follow-up — now implemented.)
    pub(crate) saved_dollar_bang: Value,
}

pub(crate) struct RescueHandler {
    pub(crate) handler_ip: usize,
    pub(crate) stack_depth: usize,
    pub(crate) bind_slot: Option<u16>,
    /// When true this entry was emitted by `Op::PushEnsure` and the
    /// unwinder pushes the exception onto the operand stack (rather than
    /// binding to a local). The ensure body re-raises with `Op::Raise`.
    pub(crate) is_ensure: bool,
    /// `loop_rescue_depths.len()` snapshot at the moment this handler
    /// was pushed. When an exception fires and this handler catches,
    /// the unwinder truncates `loop_rescue_depths` back to this value
    /// so that `Op::EnterLoop` entries pushed by `while` loops the
    /// exception is escaping out of don't leak. Without this, a
    /// later `BreakLoop` would consult the orphan top entry and
    /// pop the wrong number of rescue handlers / jump from the
    /// wrong join point.
    pub(crate) loop_depth_at_push: usize,
    /// `begin_rescue_depths.len()` snapshot at the moment this
    /// handler was pushed. When an exception fires and this
    /// handler catches, the unwinder truncates
    /// `begin_rescue_depths` back to this value so that
    /// `Op::EnterBegin` baselines pushed by inner begin/rescue
    /// blocks the exception is escaping out of don't leak.
    /// Without this, a later `retry` in an outer rescue body
    /// would read the stale inner baseline and truncate
    /// `rescues` to the wrong depth, leaving outer rescue
    /// handlers stranded. (Code-review #306 round 2.)
    pub(crate) begin_depth_at_push: usize,
    /// UNRESOLVED class filter for `rescue` — resolved lazily by the
    /// unwinder at MATCH time (see `Vm::resolve_rescue_filter`).
    /// CRuby evaluates a `rescue <expr>` class expression when an
    /// exception is actually in flight, not at begin entry — and
    /// eager push-time resolution made every begin/rescue prologue
    /// pay a lexical-scope walk (Vec clone + `format!` + intern +
    /// class-table probes per enclosing scope, per call): rubocop's
    /// `Commissioner#with_cop_error_handling` spent ~0.5µs/call on it,
    /// 25.6k calls per file walk. Now the non-raising path stores two
    /// plain words and the resolution cost rides on the raise.
    pub(crate) filter: RescueFilterSpec,
}

/// The unresolved filter carried by a `RescueHandler` (resolution
/// happens at match time — see `filter`'s doc).
#[derive(Clone, Copy)]
pub(crate) enum RescueFilterSpec {
    /// `ensure` entries — no class filter (they match unconditionally
    /// via `is_ensure`).
    None,
    /// `rescue <Name>` — the compiler-stamped SymId (possibly carrying
    /// the splat / absolute-path markers). Bare `rescue` stamps
    /// `StandardError`. Multi-class clauses (`rescue A, B => e`) emit
    /// one handler per class, so each entry holds exactly one sym.
    Sym(crate::intern::SymId),
    /// `rescue *local` — the local slot, read at match time.
    SplatLocal(u16),
}

/// The two resolved shapes a `rescue` class filter can take. The
/// single-class form stays an `Rc` clone (no extra allocation on
/// the common path); the splat form carries the materialized list.
pub(crate) enum RescueFilter {
    /// `rescue Foo` / bare `rescue` (= StandardError) — one class.
    Class(Rc<Class>),
    /// `rescue *CONST` / `rescue *local` — match if any listed class
    /// matches. The list is the expression's Array value as of MATCH
    /// time (CRuby semantics).
    Any(Vec<Rc<Class>>),
}

pub(crate) type HostFn = dyn Fn(&[Value]) -> Result<Value, Trap>;
/// v2 host-fn closure shape. Same return/args as `HostFn`, but with
/// a leading `&HostCtx` that exposes heap reads (`resolve_array`,
/// `resolve_hash`). Introduced so embed hosts can consume the
/// heap-y `Value::Array` / `Value::Hash` shapes that the v1
/// `&[Value]`-only signature couldn't reach. See
/// `Runtime::register_fn_v2`.
pub(crate) type HostFnV2 = dyn Fn(&crate::HostCtx, &[Value]) -> Result<Value, Trap>;

/// Storage slot for either signature. Held by `Vm::host_fns` so a
/// single dispatch site can resolve a name without the embed host
/// having to pick between two maps. cext stays on the v1-only
/// type alias (`Rc<HostFn>`) — its registration path predates v2
/// and doesn't need heap reads.
pub(crate) enum HostFnSlot {
    V1(Rc<HostFn>),
    V2(Rc<HostFnV2>),
}

/// ADR 0025 Phase 4a: per-signal handler state.
/// `Default` — raise Interrupt (or whatever the signal's
///             default behavior is) at the next safe point.
/// `Ignore`  — clear the flag and resume.
/// `Block`   — invoke the stored block at the next safe
///             point (Phase 4b: re-entrant dispatch).
///
/// Stored in `Vm::signal_traps` keyed by Unix signal number.
/// `Signal.trap(name, handler)` parses inputs into one of
/// these variants and replaces the current entry (returning
/// the previous one in the same shape — `"DEFAULT"` /
/// `"IGNORE"` / a `Proc` / nil).
#[derive(Clone, Debug)]
pub(crate) enum SignalHandlerState {
    Default,
    Ignore,
    Block(crate::value::ObjId),
}

impl Clone for HostFnSlot {
    fn clone(&self) -> Self {
        match self {
            HostFnSlot::V1(f) => HostFnSlot::V1(f.clone()),
            HostFnSlot::V2(f) => HostFnSlot::V2(f.clone()),
        }
    }
}

/// ADR 0025 Phase 5b: RAII guard for `Vm::suppress_interrupt`.
/// Increments on `enter`, decrements on `Drop` — panic-safe.
///
/// Round-3 review surfaced that the hand-written
/// `vm.suppress_interrupt += 1 ... vm.suppress_interrupt -= 1`
/// pattern in `InterruptAction::deliver`'s InvokeBlock branch
/// was vulnerable to panic-induced counter leakage: if
/// `invoke_block` or the nested `dispatch_until` panics (rather
/// than returns Err), the decrement is skipped, the counter
/// stays positive forever, and SIGINT delivery is permanently
/// disabled for the Vm. This guard removes that hazard.
///
/// Used by:
/// 1. `vm/step.rs::InterruptAction::deliver` around the trap
///    block's re-entrant dispatch.
/// 2. `http_server.rs::FiberResponseBody::drop` around
///    `invoke_body_close` — ADR 0023 Risk #1 mitigation.
/// 3. Future: `at_exit` drain (ADR 0025 Phase 5b extension),
///    `ensure` block executor, any other "must-complete
///    cleanup" path.
// `#[allow(dead_code)]` — the guard's only callers gate themselves
// behind `cfg(unix)` (signal-hook is unix-only). The non-unix
// builds (wasm32-wasip1, Windows) compile the type for API
// uniformity but never instantiate it.
#[allow(dead_code)]
pub(crate) struct SuppressInterruptGuard<'a> {
    pub(crate) vm: &'a mut Vm,
}

impl<'a> SuppressInterruptGuard<'a> {
    /// Increment `suppress_interrupt`, returning a guard whose
    /// Drop decrements. The dispatch-loop safe-point check
    /// reads `suppress_interrupt == 0` and skips delivery
    /// when nonzero — so deliveries land at the next safe
    /// point AFTER this guard drops.
    #[allow(dead_code)]
    pub(crate) fn enter(vm: &'a mut Vm) -> Self {
        vm.suppress_interrupt = vm.suppress_interrupt.saturating_add(1);
        Self { vm }
    }
}

impl Drop for SuppressInterruptGuard<'_> {
    fn drop(&mut self) {
        // `saturating_sub` so a logic bug somewhere that
        // double-decrements doesn't underflow into u32::MAX
        // (which would also disable SIGINT delivery forever
        // — but more silently than a panic). The matched
        // `saturating_add` in `enter` keeps the counter
        // bounded in the other direction.
        self.vm.suppress_interrupt = self.vm.suppress_interrupt.saturating_sub(1);
    }
}

/// ADR 0025 deferred follow-up: RAII guard for `Vm::cext_depth`.
/// Wrapped around each `cext_dispatch` invocation that crosses
/// into a C extension's exported function. `Drop` decrements
/// even on panic / longjmp-via-`invoke_with_raise`-error path,
/// so a cext that raises or otherwise unwinds doesn't leave the
/// counter stuck >0 (which would permanently disable
/// `Fiber.yield` for the rest of the Vm's lifetime).
///
/// `Fiber.yield`'s guard (`vm/fiber.rs::__rubyrs_fiber_yield`)
/// reads `cext_depth > 0` and raises `FiberError` rather than
/// unwinding the Rust stack through C frames that don't expect
/// Ruby control flow. The guard is `_fiber`-gated; without
/// `_fiber` the counter ticks but has no consumer (counter is
/// always-present so the cext bridge doesn't need feature-gated
/// code paths).
#[cfg(feature = "cext")]
pub(crate) struct CextDepthGuard<'a> {
    pub(crate) vm: &'a mut Vm,
}

#[cfg(feature = "cext")]
impl<'a> CextDepthGuard<'a> {
    pub(crate) fn enter(vm: &'a mut Vm) -> Self {
        vm.cext_depth = vm.cext_depth.saturating_add(1);
        Self { vm }
    }
}

#[cfg(feature = "cext")]
impl Drop for CextDepthGuard<'_> {
    fn drop(&mut self) {
        self.vm.cext_depth = self.vm.cext_depth.saturating_sub(1);
    }
}

/// ADR 0024 Phase A: RAII guard for `Vm::yield_recursion_depth`.
/// `enter` increments + range-checks against `max_yield_recursion`
/// (returns `ResourceExhausted` Trap if exceeded); `Drop`
/// decrements unconditionally — panic-safe.
///
/// Mirrors `SuppressInterruptGuard`'s shape (round-3 v7
/// review pattern) so a panic in the synchronous `Op::Yield`
/// wrapper's nested `dispatch_until` can't permanently bump
/// the counter and falsely trip the cap on subsequent yields.
#[allow(dead_code)] // wired in Phase A.1
pub(crate) struct YieldDepthGuard<'a> {
    pub(crate) vm: &'a mut Vm,
}

impl<'a> YieldDepthGuard<'a> {
    #[allow(dead_code)] // wired in Phase A.1
    pub(crate) fn enter(vm: &'a mut Vm) -> Result<Self, Trap> {
        let new = vm.yield_recursion_depth.saturating_add(1);
        if let Some(cap) = vm.max_yield_recursion
            && new > cap {
            return Err(vm.trap(crate::error::RubyError::ResourceExhausted {
                msg: format!(
                    "yield recursion depth exceeded ({new} > {cap})"
                ),
            }));
        }
        vm.yield_recursion_depth = new;
        Ok(Self { vm })
    }
}

impl Drop for YieldDepthGuard<'_> {
    fn drop(&mut self) {
        // `saturating_sub` for symmetry with SuppressInterruptGuard
        // (defensive against double-drop bugs).
        self.vm.yield_recursion_depth = self.vm.yield_recursion_depth.saturating_sub(1);
    }
}

/// Side-channel record of the most recent successful regex match.
/// Holds owned strings so the GC need not walk it; the cost is
/// one `.to_string()` per capture group on each successful match.
/// `caps[i]` is the i-th *parenthesised* group (1-indexed via
/// `$1` etc.); `None` means the group did not participate. The
/// vector length is always `re.captures_len() - 1` after a hit.
#[cfg(feature = "regex")]
#[derive(Debug, Clone)]
pub(crate) struct LastMatch {
    pub(crate) whole: String,
    pub(crate) caps: Vec<Option<String>>,
    /// Original input string the match was performed against, plus
    /// the byte span of the whole match within it. Required to back
    /// `` $` `` (pre-match) and `$'` (post-match) — those return
    /// slices of the input that we'd otherwise have to recompute.
    /// `pre_match` is `input[..m_start]`, `post_match` is
    /// `input[m_end..]`.
    pub(crate) input: String,
    pub(crate) m_start: usize,
    pub(crate) m_end: usize,
    /// `(name, matched | None)` for each NAMED capture group, so
    /// `$~[:name]` / `$~["name"]` (and any `MatchData` re-materialised
    /// from `$~`, e.g. a `StringScanner`'s `@src[:name]`) can resolve
    /// named groups. Empty for patterns without named captures.
    pub(crate) named: Vec<(String, Option<String>)>,
    /// Byte spans of capture groups 1..N within `input` (parallel to
    /// `caps`; `None` for a group that didn't participate). Backs
    /// `MatchData#begin`/`#end`/`#offset` (and the `byte*` variants) for
    /// group indices — the whole match's span is `(m_start, m_end)`.
    /// Empty when the producing path didn't carry span info (group
    /// offsets then read as nil there).
    pub(crate) group_spans: Vec<Option<(usize, usize)>>,
    /// Names of capture groups 1..N in index order (parallel to `caps`):
    /// `Some(name)` for `(?<name>…)`, `None` for an unnamed group. Lets
    /// a re-materialised `MatchData` resolve `#begin(:name)` to the
    /// group's position. Empty for patterns without named captures.
    pub(crate) cap_names: Vec<Option<String>>,
    /// `Some` only when the match ran against an ASCII-8BIT (BINARY)
    /// subject. Carries the raw subject bytes + per-group byte spans so
    /// positional captures can be rebuilt byte-faithfully (and tagged
    /// ASCII-8BIT) instead of through `caps`'s lossy `from_utf8_lossy`
    /// strings, which mangle invalid bytes to U+FFFD. `None` leaves the
    /// proven UTF-8 path (`whole`/`caps`/`input`) completely untouched.
    /// rack's multipart parser scans a binary body with a StringScanner
    /// and reads `@sbuf[1]` (the content-disposition head, which may
    /// contain an invalid filename byte).
    pub(crate) binary: Option<BinaryCaps>,
}

/// Byte-faithful capture data for a BINARY-subject match — see
/// `LastMatch::binary`. Holds the raw subject and the byte span of
/// each capture group (parallel to `LastMatch::caps`), so a consumer
/// can slice the original bytes and tag the result ASCII-8BIT.
#[cfg(feature = "regex")]
#[derive(Debug, Clone)]
pub(crate) struct BinaryCaps {
    pub(crate) input: Box<[u8]>,
    pub(crate) group_spans: Vec<Option<(usize, usize)>>,
}

/// Per-defining-module list of `(target_class, refinement_holder)` pairs
/// recorded by `refine`; see `Vm::module_refinements`.
pub(crate) type RefinementList = Vec<(std::rc::Rc<Class>, std::rc::Rc<Class>)>;

/// Env-gated (`RUBYRS_JIT_STATS`) counters for the native-JIT method paths:
/// compile attempts / successes / pre-gate declines per variant family, and
/// per-(proto, family) native EXECUTION counts + deopts. Zero-cost when off
/// (every update is behind the `jit_stats_on` bool). Dumped to stderr on
/// `Runtime` drop by `Vm::dump_jit_stats`.
#[cfg(feature = "jit-native")]
#[derive(Default)]
pub(crate) struct JitStats {
    /// Indexed by family: 0=int 1=poly 2=fparam 3=objparam 4=objparam2
    /// 5=value 6=zeroarg 7=tier2 8=t2lite. `[attempts, ok, pregate_declines]`.
    pub(crate) compile: [[u64; 3]; 9],
    /// (proto_idx, family) → (native calls, deopts).
    pub(crate) exec: crate::intern::FxHashMap<(usize, u8), (u64, u64)>,
}

#[cfg(feature = "jit-native")]
pub(crate) const JIT_FAM_NAMES: [&str; 9] =
    ["int", "poly", "fparam", "objparam", "objparam2", "value", "zeroarg", "tier2", "t2lite"];

/// Second-arg descriptor for the 2-arg (`objparam2`) native dispatch helper
/// (`Vm::jit_run_objparam2`): the compiled param1 is either an Int value or a
/// Hash `ObjId` (`walk(node, counts)`), per `NativeProto::param2_hash`.
#[cfg(feature = "jit-native")]
#[derive(Clone, Copy)]
pub(crate) enum ObjP2Arg {
    Int(i64),
    Hash(crate::value::ObjId),
}

/// `Vm::jit_flags` bit: the proto's ZERO-arg verdict is settled and dead
/// (declined, or breaker-killed) — the explicit-recv argc==0 serving block
/// skips its map probe entirely on one dense `Vec<u8>` read.
#[cfg(feature = "jit-native")]
pub(crate) const JFLAG_NO_ZEROARG: u8 = 1;
/// `Vm::jit_flags` bit: ALL THREE 1-arg verdicts (int / value / objparam) are
/// settled and dead — serving probes AND hook routing are skipped on one read.
/// (The lazy Float specialization is intentionally NOT part of this bit: the
/// Float sub-arm stays reachable behind a stack-value `matches!`, no map probe.)
#[cfg(feature = "jit-native")]
pub(crate) const JFLAG_NO_ONEARG: u8 = 2;
/// `Vm::jit_flags` bit: the 2-arg (`objparam2`) verdict is settled and dead.
#[cfg(feature = "jit-native")]
pub(crate) const JFLAG_NO_OBJP2: u8 = 4;
/// `Vm::jit_flags` bit: the TIER-2 (frame-keeping direct-threaded, ADR 0037)
/// verdict is settled and dead — declined at admission or filtered by the
/// `RUBYRS_JIT_TIER2_ONLY` allowlist.
#[cfg(feature = "jit-native")]
pub(crate) const JFLAG_NO_TIER2: u8 = 8;
/// `Vm::jit_flags` bit: a TIER-2 body is compiled and present in
/// `Vm::t2_protos` — serve without probing the hotness map.
#[cfg(feature = "jit-native")]
pub(crate) const JFLAG_TIER2_HAS: u8 = 16;
/// Wave-4: a frame-lite entry EXISTS (and has not been breaker-killed) in
/// `t2_lite_ptrs`. The serve sites gate on this dense byte (usually already
/// in cache from the tier-2 flow) before touching the 24-byte-entry lite
/// table, so the non-serving fixed-arity fast path pays one AND, not a
/// second table probe. Cleared by the bail-streak breaker.
#[cfg(feature = "jit-native")]
pub(crate) const JFLAG_TIER2_LITE: u8 = 32;
/// Set when the proto's tier-2 compile produced a LITE-BLOCK entry
/// (`T2Proto::lite_blk_ptr`, ADR 0037 block-frame residue) — the block
/// serve sites gate their frameless path on this dense byte before
/// touching `t2_lite_blk_ptrs`.
#[cfg(feature = "jit-native")]
pub(crate) const JFLAG_TIER2_LITEBLK: u8 = 64;

/// Tier-2 hotness threshold (ADR 0037 wave 2, compile-cost control): a
/// proto's frame must be entered `BASE + PER_OP × body_ops` times before it
/// is compiled. Wave 1's flat threshold of 8 made a single `f1.rb` RuboCop
/// run pay ~270ms compiling 848 protos (a +16% e2e regression) and a big1
/// run ~640ms/2446 protos — compile cost is ~linear in body size (~8µs/op)
/// while per-entry native savings are tens-to-hundreds of ns, so payback
/// needs O(1000) entries. Scaling the threshold with body size makes short
/// CLI runs compile almost nothing while daemon/batch/hot-loop workloads
/// still compile everything that matters (a proto hot enough to pay back
/// reaches the threshold quickly). Env overrides for experiments and for
/// exercising the tier in tests: `RUBYRS_JIT_TIER2_THRESHOLD` (absolute:
/// sets BASE and zeroes PER_OP), `RUBYRS_JIT_TIER2_BASE`,
/// `RUBYRS_JIT_TIER2_PEROP`.
#[cfg(feature = "jit-native")]
const T2_THRESHOLD_BASE_DEFAULT: u32 = 2048;
#[cfg(feature = "jit-native")]
// Wave 3: the inline lowering grew per-op compile cost ~2.4x (guards +
// slow-edge blocks), so the threshold scales to match — payback needs
// entries proportional to compile cost (measured ~19us/op vs wave-2's
// ~8us/op). Re-measured on f1 e2e: 1024+16/op left a +2.6% one-shot
// regression (65.7ms bill); 2048+64/op is e2e-NEUTRAL (30.5ms bill, 50
// protos, 629k IC-fast serves) while hot workloads (fib, the walk's
// 100k+-call bodies) still compile within their first few thousand
// entries.
const T2_THRESHOLD_PER_OP_DEFAULT: u32 = 64;
/// Tier-2 native-nesting cap: each nested native body adds a Rust stack
/// segment (native fn + helper + dispatch_until + step); deeper Ruby
/// recursion falls back to the flat interpreter loop, which has no Rust-stack
/// cost per Ruby frame. Shared with the lite→lite native chains (LITE
/// t2_call): a chain past the cap materializes and continues interpreted.
#[cfg(feature = "jit-native")]
pub(crate) const T2_MAX_NATIVE_DEPTH: u32 = 96;
/// Wave-4 frame-lite bail-streak breaker: this many CONSECUTIVE
/// materialize-bails (no completed frameless serve in between) disable the
/// proto's lite entry — a chronic shape mismatch (e.g. a Float-operand
/// predicate whose Int guards never hold) pays entry + materialize per call
/// for nothing. A completed serve resets the streak, so mixed workloads
/// with occasional bails keep serving.
#[cfg(feature = "jit-native")]
const T2_LITE_KILL_STREAK: u8 = 32;

/// A SUSPENDED frameless (frame-lite) activation whose native code is
/// mid-way through a lite→lite call (ADR 0037 wave-4 follow-on, LITE
/// t2_call). Pushed by the lite call helper around the nested callee
/// invocation; consumed either by popping (the callee completed
/// frameless) or by the materialize cascade: when ANY deeper activation
/// materializes, every pending record is drained OUTERMOST-FIRST — each
/// pushing its deferred frame with `resume_ip` stamped AFTER its call op
/// (the call has happened; the callee's frame sits above) — so the frame
/// order on `vm.frames` is exactly the interpreter's. `slot` points into
/// the suspended activation's native spill slot (a Rust/Cranelift stack
/// address, valid while its native frame is alive — which spans the whole
/// nested call by construction); the slot values are read only at drain
/// time, while the activation is suspended, so they cannot change
/// underneath. `dc` is the activation's `defining_class` (captured from
/// the resolving `Method` at serve time — the deferred-push twin of the
/// serve sites' own `m.defining_class` stamp).
#[cfg(feature = "jit-native")]
pub(crate) struct T2LitePending {
    pub(crate) slot: *const i64,
    pub(crate) pidx: usize,
    pub(crate) argc: usize,
    pub(crate) n_locals: usize,
    pub(crate) n_pop: usize,
    pub(crate) trunc: usize,
    pub(crate) self_w0: i64,
    pub(crate) self_w1: i64,
    pub(crate) resume_ip: usize,
    pub(crate) dc: Option<std::rc::Rc<crate::value::Class>>,
    /// LITE-BLOCK caller: the suspended activation's BlockHandle id + 1
    /// (0 = a method activation). A cascade drain pushes a BLOCK frame
    /// (`push_lite_block_frame`) for such a record.
    pub(crate) blk: i64,
    /// The suspended block's own-region start (0 for methods).
    pub(crate) ps: usize,
}

/// Raw ingredients of a constant's definition location, resolved to
/// a (file, line) pair only when `Module#const_source_location` is
/// queried. `source` is the exact text the defining op executed
/// against (captured at stamp time so a later `load` of the same
/// path can't skew the answer); `byte_offset` is the defining op's
/// span offset. See the `Vm::const_source_locations` field doc for
/// why the line is NOT resolved eagerly at define time.
#[derive(Clone)]
pub(crate) struct ConstLoc {
    pub(crate) file: std::rc::Rc<str>,
    pub(crate) source: std::rc::Rc<str>,
    pub(crate) byte_offset: u32,
    /// Memoized offset→line answer, filled by the first `line()`
    /// query so repeat queries are O(1) instead of re-scanning
    /// `source`. Sound because the ingredients are immutable: the
    /// `source` Rc is pinned at stamp time, and `remove_const` +
    /// redefine stamps a FRESH `ConstLoc` (fresh empty cell), so a
    /// stamped entry's answer can never change. NOTE: call `line()`
    /// through the MAP ENTRY (`const_source_locations.get(..)`),
    /// not a clone — `Cell` is copied by value, so memoizing into a
    /// temporary clone is discarded with it.
    line: std::cell::Cell<Option<u32>>,
}

impl ConstLoc {
    pub(crate) fn new(file: std::rc::Rc<str>, source: std::rc::Rc<str>, byte_offset: u32) -> Self {
        ConstLoc { file, source, byte_offset, line: std::cell::Cell::new(None) }
    }

    /// 1-based line of the defining op — the value the old eager
    /// stamp stored. The first call scans `source` up to
    /// `byte_offset` (identical arithmetic to the eager path via
    /// `error::line_col`) and memoizes; later calls return the
    /// cached line.
    pub(crate) fn line(&self) -> u32 {
        if let Some(l) = self.line.get() {
            return l;
        }
        let l = crate::error::line_col(&self.source, self.byte_offset).0;
        self.line.set(Some(l));
        l
    }
}

/// Backing map for [`Vm::nfa_stats`] — key is `(method name, argc
/// passed, packed param shape, no_recv)`; see the field doc for the
/// shape-bit layout.
pub(crate) type NfaStatsMap = FxHashMap<(SymId, u16, u32, bool), u64>;

/// Backing map for [`Vm::t2_fb_stats`] — key is `(reason code, method
/// name, receiver-shape code, min(argc,15))`; see the field doc.
#[cfg(feature = "jit-native")]
pub(crate) type T2FbStatsMap = FxHashMap<(u8, SymId, u8, u8), u64>;

/// Backing map for [`Vm::t2_op_stats`] — key is `(op variant tag,
/// call name when one exists)`; see the field doc.
#[cfg(feature = "jit-native")]
pub(crate) type T2OpStatsMap = FxHashMap<(String, Option<SymId>), u64>;

pub(crate) struct Vm {
    pub(crate) protos: Vec<Proto>,
    #[cfg(feature = "jit-native")]
    pub(crate) jit_stats_on: bool,
    #[cfg(feature = "jit-native")]
    pub(crate) jit_stats: JitStats,
    /// Per-(proto, family) DISPATCH-deopt counts, feeding the circuit-breaker
    /// (`jit_note_deopt`): only bumped on the deopt path (the expensive path —
    /// the interpreter re-runs the body right after), so native successes pay
    /// nothing for it.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_deopt_count: crate::intern::FxHashMap<(usize, u8), u32>,
    /// Dense per-proto NEGATIVE cache for JIT dispatch serving (`JFLAG_*`):
    /// the walk-shaped workload dispatches thousands of methods whose every
    /// variant declined, and the serving blocks were paying several
    /// FxHashMap probes per CALL to re-discover that. One `Vec<u8>` read
    /// answers the settled-dead common case. Bits only ever turn ON (a dead
    /// verdict never revives; a method redefinition allocates a NEW proto,
    /// which starts with a zeroed flag byte). Indexed by `proto_idx`, grown on
    /// demand (`jit_flags_set`).
    #[cfg(feature = "jit-native")]
    pub(crate) jit_flags: Vec<u8>,
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeProto>>,
    /// ZERO-arg method NativeProtos compiled for the `jit_obj_call` PIC path (ADR 0034
    /// Step 1). Kept SEPARATE from `jit_native` so a 0-arg proto can NEVER be served at
    /// a 1-arg dispatch site (B1 / explicit-recv look up `jit_native`), which would
    /// swallow the wrong-arity ArgumentError. Only `jit_obj_call` (argc=0) reads this.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_zeroarg: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeProto>>,
    /// OBJECT-param specialization of a 1-arg method (`def weigh(node); node.value*2;
    /// end` called with an Object arg): the param binds as a `*const Value` receiver
    /// pointer (ADR 0034 Step 1, param-receiver). Kept separate from `jit_native` (Int
    /// param) since the ABI differs (pointer vs Int value). Keyed by proto.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_objparam: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeProto>>,
    /// 2-ARG method specialization (`def walk(node, acc); …; walk(child, acc); end`,
    /// ADR 0034 piece 1+8): param0 binds as a `*const Value` Object receiver pointer,
    /// param1 as an Int. The C ABI is `(vm, self, ptr, i64) -> NRet` (called via
    /// `NativeProto::call2`); a 2-arg self-recursion lowers to a native 4-arg self-call.
    /// Kept separate from the 1-arg maps since the arity/ABI differ. Keyed by proto.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_objparam2: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeProto>>,
    /// A pooled, GC-rooted SCRATCH Hash reused by the 2-arg Hash-param JIT dispatch
    /// (`walk(node, counts)`, ADR 0034 pieces 2-4). The native walk mutates this scratch
    /// (a re-seeded clone of the real `counts`) instead of `counts` itself; on full
    /// success the dispatch moves the scratch's pairs back into `counts`, on a deopt it's
    /// discarded — so a deopt-after-write can't leak/double-count. Pooled (alloc'd once,
    /// re-seeded per call) so the common path adds no heap allocation. GC-rooted below.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_hash_scratch: Option<crate::value::ObjId>,
    /// TIER-2 (ADR 0037): env-gated (`RUBYRS_JIT_TIER2`) frame-keeping
    /// direct-threaded baseline tier. `t2_protos` holds compiled bodies keyed
    /// by proto_idx (never removed — machine code addresses stay valid);
    /// `t2_hot` counts frame entries until `T2_COMPILE_THRESHOLD`;
    /// `t2_trap` carries a Trap across the C ABI (status 3); `t2_depth` is
    /// the live native-nesting count (capped at `T2_MAX_NATIVE_DEPTH`).
    #[cfg(feature = "jit-native")]
    pub(crate) jit_tier2_on: bool,
    /// Optional method-name allowlist (`RUBYRS_JIT_TIER2_ONLY=a,b,c`) for
    /// controlled per-method A/B runs; `None` = admit everything eligible.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_tier2_only: Option<std::collections::HashSet<String>>,
    #[cfg(feature = "jit-native")]
    pub(crate) t2_protos: crate::intern::FxHashMap<usize, crate::jit_tier2::T2Proto>,
    /// Dense proto_idx → compiled-entry table (parallel to `t2_protos`, which
    /// OWNS the modules): the per-serve lookup is one bounds-checked Vec read
    /// instead of an FxHashMap probe (~1M serves per rubocop walk).
    #[cfg(feature = "jit-native")]
    pub(crate) t2_ptrs: Vec<Option<extern "C" fn(*mut Vm) -> i64>>,
    #[cfg(feature = "jit-native")]
    pub(crate) t2_hot: crate::intern::FxHashMap<usize, u32>,
    #[cfg(feature = "jit-native")]
    pub(crate) t2_trap: Option<crate::error::Trap>,
    #[cfg(feature = "jit-native")]
    pub(crate) t2_depth: u32,
    /// Total tier-2 Cranelift compile time (stats-gated; ns).
    #[cfg(feature = "jit-native")]
    pub(crate) t2_compile_ns: u64,
    /// Wave-2 (`RUBYRS_JIT_TIER2_NOCALL`): compile call ops + `Return`
    /// through the generic helper (the wave-1 tier) for controlled A/B.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_tier2_nocall: bool,
    /// Adaptive compile-threshold knobs (see `T2_THRESHOLD_BASE_DEFAULT`).
    #[cfg(feature = "jit-native")]
    pub(crate) t2_threshold_base: u32,
    #[cfg(feature = "jit-native")]
    pub(crate) t2_threshold_per_op: u32,
    /// Wave-2 `t2_call` counters (stats-gated): `[0]` IC-fast serves (the
    /// dedicated helper served without the do_call cascade), `[1]` fallbacks
    /// to the full cascade, `[2]` native→native entries (a tier-2 body run
    /// while already inside tier-2 native code, i.e. `t2_depth > 0`).
    #[cfg(feature = "jit-native")]
    pub(crate) t2_call_stats: [u64; 3],
    /// Wave-5 (`RUBYRS_JIT_TIER2_NOBLOCK`): disable BLOCK-proto serving
    /// (`t2_enter_block` becomes a no-op) for controlled blocks-off A/B runs;
    /// method-frame serving is unaffected.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_tier2_noblock: bool,
    /// Wave-5 block-serving counters (stats-gated): `[0]` block invocations
    /// that reached a serving hook (the invoke_block-family sites), `[1]`
    /// invocations served natively (compiled block body ran), `[2]` native
    /// serves that came from the `Op::Yield` arm (native-yield count).
    #[cfg(feature = "jit-native")]
    pub(crate) t2_block_stats: [u64; 3],
    /// Wave-3 (`RUBYRS_JIT_TIER2_NOINLINE`): disable the inline op lowering
    /// (reproduces the wave-2 tier — per-op helpers + IC-fast calls) for
    /// controlled A/B.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_tier2_noinline: bool,
    /// Wave-3 item 3: per-call-site settled-verdict bytes for the `t2_call`
    /// fast probes, dense by `cache_id`. Counts consecutive fast-probe
    /// declines; ≥ `T2_SITE_SETTLE` short-circuits the probe (with a
    /// ~1/1024 periodic retry keyed off `op_counter`). Reset to 0 on any
    /// fast serve. Grows on demand at the decline path only.
    #[cfg(feature = "jit-native")]
    pub(crate) t2_site_verdict: Vec<u8>,
    /// Wave-3 backward-branch poll gate: nonzero when fuel or a wall-clock
    /// deadline is active, so compiled loop back-edges call the poll helper
    /// (which charges `check_fuel`). Recomputed at every tier-2 serve entry
    /// (fuel/deadline activation only changes between evals). Read INLINE
    /// by generated code via its baked field offset, alongside
    /// `control_signals` and the interrupt flag.
    #[cfg(feature = "jit-native")]
    pub(crate) t2_poll_flags: u8,
    /// Wave-4 FRAME-LITE entries, dense by proto_idx: `(fn, argc)` when the
    /// body compiled a frameless variant (see `jit_tier2::T2LiteFn`). Served
    /// at the fixed-arity dispatch fast paths BEFORE any arg bind / frame
    /// push; `None` = not compiled (yet) or killed by the bail-streak
    /// breaker. The machine code is owned by `t2_protos`' module.
    #[cfg(feature = "jit-native")]
    pub(crate) t2_lite_ptrs: Vec<Option<(crate::jit_tier2::T2LiteFn, u16)>>,
    /// Consecutive materialize-bail counter per proto (dense): a lite serve
    /// that completes resets it; `T2_LITE_KILL_STREAK` consecutive bails
    /// disable the lite entry (each bail costs a wasted native entry plus
    /// the materialize on top of the interpreted run — chronic mismatches,
    /// e.g. an always-non-Int operand shape, must settle to the framed
    /// path). Mixed workloads with occasional bails never accumulate.
    #[cfg(feature = "jit-native")]
    pub(crate) t2_lite_streak: Vec<u8>,
    /// Frame-lite counters: `[0]` native serves that completed frameless
    /// (stats-gated), `[1]` materialize-bails (counted in the helper),
    /// `[2]` breaker kills.
    #[cfg(feature = "jit-native")]
    pub(crate) t2_lite_stats: [u64; 3],
    /// LITE-BLOCK entries, dense by proto:
    /// `(entry, param_start, n_params_bound, is_rest)`. The
    /// `param_start`/`n_params` copies double as the serve-site guard —
    /// the invoking BlockHandle must match them exactly (paranoia against a
    /// proto ever being shared across CreateBlock sites). `is_rest` = the
    /// rest-only `|*a|` entry: `n_params_bound` is 1 (the pre-allocated
    /// rest Array) and the handle guard becomes
    /// `n_params == 0 && rest_slot == Some(param_start)`.
    #[cfg(feature = "jit-native")]
    pub(crate) t2_lite_blk_ptrs: Vec<Option<(crate::jit_tier2::T2LiteBlkFn, u16, u16, bool)>>,
    /// `RUBYRS_JIT_TIER2_NOLITEBLK`: disable lite-block SERVING (the
    /// sibling still compiles) for controlled A/B.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_tier2_noliteblk: bool,
    /// Stats-gated LITE-BLOCK counters: [0] frameless entries,
    /// [1] completed frameless (DONE). Materialize-bails land in
    /// `t2_lite_stats[1]` like every lite materialize.
    #[cfg(feature = "jit-native")]
    pub(crate) t2_lite_blk_stats: [u64; 2],
    /// TEMPORARY census (stats-gated): rest-only-block (`|*a|`) invocation
    /// argc distribution — [argc 0..=4, 5+] — across the ib1 fast arm and
    /// the general binder.
    #[cfg(feature = "jit-native")]
    pub(crate) restblk_census: [u64; 6],
    /// TEMPORARY census (stats-gated): rest-only-block invocations by proto.
    #[cfg(feature = "jit-native")]
    pub(crate) restblk_census_by: crate::intern::FxHashMap<usize, u64>,
    /// Stats-gated: 1-arg `&:sym` sym-proc block invocations served as a
    /// direct `arg.sym()` dispatch (no rest Array / block frame).
    #[cfg(feature = "jit-native")]
    pub(crate) symproc_serves: u64,
    /// `RUBYRS_JIT_TIER2_NOLITE`: disable wave-4 frame-lite compilation and
    /// serving (reproduces the wave-3/5 tier) for controlled A/B.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_tier2_nolite: bool,
    /// LITE t2_call (wave-4 follow-on): suspended frameless activations
    /// mid-way through a lite→lite native call, outermost first. Non-empty
    /// ONLY inside a nested lite chain; drained (outward-in) by any
    /// materialize. See `T2LitePending`.
    #[cfg(feature = "jit-native")]
    pub(crate) t2_lite_pending: Vec<T2LitePending>,
    /// The `defining_class` for the INNERMOST live lite activation (its
    /// pending record holds the outer ones'): set by every lite serve
    /// entry (`t2_lite_run` / the lite call helper's chain hand-off),
    /// consumed by the deferred frame push so a materialized lite frame
    /// carries exactly the `defining_class` the interpreter's push would
    /// have (read by `do_call`'s Nil-self bare-call gates; `super`/cvar
    /// readers stay unreachable — those ops decline lite admission).
    #[cfg(feature = "jit-native")]
    pub(crate) t2_lite_dc: Option<std::rc::Rc<crate::value::Class>>,
    /// LITE t2_call counters (stats-gated): `[0]` call ops served
    /// frameless in place (getter/zeroarg/native-family/rest-pred/
    /// fast-prim), `[1]` call ops that materialized (conservative decline
    /// or mid-chain cascade), `[2]` lite→lite native chain serves,
    /// `[3]` frames pushed by cascade drains, `[4]` IC-hit const-chain
    /// reads served frameless.
    #[cfg(feature = "jit-native")]
    pub(crate) t2_lite_call_stats: [u64; 5],
    /// FLOAT-param specialization of a 1-arg method (`def scale(n); n*1.5; end`
    /// called with a Float arg): the param binds as Float, the i64 arg carries f64
    /// bits. Leaf methods only (no cross-calls — those decline). Keyed by proto,
    /// parallel to `jit_native`, so a method called with both Int and Float args
    /// gets both specializations and dispatch picks by the arg's runtime type.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_fparam: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeProto>>,
    /// Polymorphic inline cache (ADR 0034): a cross-call-bearing method
    /// (`guard_class != 0`) compiled for its FIRST receiver class lives in
    /// `jit_native`; subsequent receiver classes get their own variant here,
    /// keyed `(proto_idx, class_ptr)`, each compiled with its callees resolved
    /// on THAT class and guarded to it — so a polymorphic call site runs native
    /// for every class instead of deopting for all but the first. Correct by
    /// construction: a variant only ever runs for the class it was compiled for.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_poly:
        crate::intern::FxHashMap<(usize, usize), Option<crate::jit_native::NativeProto>>,
    #[cfg(feature = "jit-native")]
    pub(crate) jit_value: crate::intern::FxHashMap<usize, Option<crate::jit_native::ValueProto>>,
    /// Native-compiled 1-param BLOCKS, keyed by block proto_idx (B5). A Rust
    /// iterator driver (`step_block1`) calls the native block instead of
    /// re-entering the interpreter when the block is a pure int function of its
    /// param. `Some(None)` = declined (not native-compilable).
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_block: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeProto>>,
    /// Per block-proto cache of compiled whole-loop `Array#sum { block }` drivers
    /// (ADR 0034 layer 3): the full iteration runs native, calling the native
    /// block per element with no per-element interpreter re-entry.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_sum_loop: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeLoop>>,
    /// B4 (ADR 0034) — `objs.sum { |o| o.method(CONST) }` whole-loop object-method
    /// dispatch drivers, keyed by `(block_proto, element_class_ptr, callee_proto)`.
    /// The callee's native address + class guard + const arg are baked in, so the
    /// key carries the callee proto: a method redefinition (new proto) recompiles
    /// instead of calling a stale address.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_objmethod_sum_loop:
        crate::intern::FxHashMap<(usize, usize, usize), Option<crate::jit_native::NativeLoop>>,
    /// Per block-proto cache of compiled whole-loop `Array#map { block }` drivers
    /// (ADR 0034 layer 3): the full map runs native, filling a pre-sized result.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_map_loop: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeLoop>>,
    /// Predicate-mode compilation of a block proto (a `Bool` result materialised
    /// as i64 0/1), keyed separately from the value-mode `jit_native_block` since
    /// the two compile the same proto differently. Used by count/select/...
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_block_pred: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeProto>>,
    /// Per block-proto cache of whole-loop `Array#count { pred }` drivers — a sum
    /// loop accumulating the predicate block's 0/1 results.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_count_loop: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeLoop>>,
    /// Per-(block proto, keep-polarity) cache of whole-loop `select` / `reject`
    /// drivers — a predicate loop pushing the matching elements. `true` = select
    /// (keep truthy), `false` = reject (keep falsy).
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_filter_loop: crate::intern::FxHashMap<(usize, bool), Option<crate::jit_native::NativeLoop>>,
    /// Per block-proto cache of whole-loop `find` / `detect` drivers — a predicate
    /// loop that pushes the first match into a capacity-1 array and early-exits.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_find_loop: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeLoop>>,
    /// 2-param block compilation (inject/reduce `|acc, x|`), cached separately.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_block2: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeProto>>,
    /// Per block-proto cache of whole-loop `inject` / `reduce` drivers.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_inject_loop: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeLoop>>,
    /// Float-accumulator 2-param `inject(init){|s,x| ...}` block + loop, keyed by
    /// (block proto, int_elem): `false` = Float elements, `true` = Int elements. The
    /// accumulator is always a Float (opaque bits); only the element reader differs.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_block2_finject: crate::intern::FxHashMap<(usize, bool), Option<crate::jit_native::NativeProto>>,
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_finject_loop: crate::intern::FxHashMap<(usize, bool), Option<crate::jit_native::NativeLoop>>,
    /// `each`-accumulator block compilation (`{ |x| total += x }`, acc bound to a
    /// captured slot), cached separately from the 1-/2-param block caches.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_block_acc: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeProto>>,
    /// Per block-proto cache of whole-loop `each`-accumulator drivers (reuses the
    /// inject loop shape over the acc-bound block).
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_each_acc_loop: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeLoop>>,
    /// `each_with_object` block compilation (`{ |x, memo| memo << f(x) }`, memo
    /// bound to a scratch Array), cached separately.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_block_eachobj: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeProto>>,
    /// Per block-proto cache of whole-loop `each_with_object` drivers.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_eachobj_loop: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeLoop>>,
    /// Float-element `each_with_object` (`floats.each_with_object(m) { |x,m| m << f(x) }`):
    /// separate block (Float element) + loop (Float reader) caches.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_block_eachobj_f: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeProto>>,
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_eachobj_loop_f: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeLoop>>,
    /// Hash-accumulator `each_with_object(Hash.new(0)) { |x,h| h[k] += v }` (sum-by-key):
    /// block + loop caches keyed by `(proto, float_elem, float_val)` — the element reader
    /// varies with `float_elem`, and the value/accumulator kind (Int vs Float Hash value)
    /// with `float_val`. The key kind (Int/Float) is intrinsic to the proto, so it needs
    /// no cache dimension.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_block_eachobjhash: crate::intern::FxHashMap<(usize, bool, bool), Option<crate::jit_native::NativeProto>>,
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_eachobjhash_loop: crate::intern::FxHashMap<(usize, bool, bool), Option<crate::jit_native::NativeLoop>>,
    /// Per block-proto cache of whole-loop `group_by` drivers (the value-mode key
    /// block is shared with `jit_native_block`).
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_groupby_loop: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeLoop>>,
    /// `each_with_index`-accumulator block compilation (`{ |x, i| total += f(x, i)
    /// }`, acc bound to a captured slot, index as a 3rd block arg). Keyed by (block
    /// proto, float_acc): `false` = Int accumulator, `true` = Float accumulator
    /// (Int element, `t += x*1.5 + i`).
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_block_eachidx_k: crate::intern::FxHashMap<(usize, bool), Option<crate::jit_native::NativeProto>>,
    /// Per-(block proto, float_acc) cache of whole-loop `each_with_index`-accumulator drivers.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_eachidx_loop_k: crate::intern::FxHashMap<(usize, bool), Option<crate::jit_native::NativeLoop>>,
    /// Float-element value-block compilation (the element param binds as `Float`),
    /// for the Float-element drivers (`sum`/`map`). Cached separately from the Int blocks.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_block_float: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeProto>>,
    /// Per block-proto cache of whole-loop Float `sum` drivers.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_floatsum_loop: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeLoop>>,
    /// Per block-proto cache for the INT-element/Float-accumulator `sum` block
    /// (`ints.sum { |x| x * 1.5 }`) — distinct compile from the Float-element block.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_block_intelem_fa: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeProto>>,
    /// Per block-proto cache of the Int-element/Float-acc `sum` loop driver.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_intelem_floatsum_loop: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeLoop>>,
    /// Float-element / Int-result `sum` (`floats.sum { x.floor }`): the block reads a
    /// Float but returns an Int (a Float->Int conversion); the loop accumulates Int.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_block_floatint: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeProto>>,
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_floatint_sum_loop: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeLoop>>,
    /// Float-element / Int-output `map` (`floats.map { x.round }`): shares the
    /// `jit_native_block_floatint` block; the loop reads Float, stores Int.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_floatint_map_loop: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeLoop>>,
    /// Float-element / Int-key `group_by` (`floats.group_by { x.floor }`): shares the
    /// `jit_native_block_floatint` key block; the loop buckets Floats under Int keys.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_floatint_groupby_loop: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeLoop>>,
    /// Float-element / Float-key `group_by` (`floats.group_by { x * 2.0 }`): shares the
    /// `jit_native_block_float` value block; buckets Floats under Float keys (eql?).
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_floatkey_groupby_loop: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeLoop>>,
    /// Per block-proto cache of the Int-element/Float-output `map` loop driver
    /// (`ints.map { |x| x*1.5 }`). Shares the `jit_native_block_intelem_fa` block.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_intelem_floatmap_loop: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeLoop>>,
    /// Per block-proto cache of whole-loop Float `map` drivers (Float in -> Float out).
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_floatmap_loop: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeLoop>>,
    /// Float each-accumulator block compilation (`{ |x| total += f(x) }`, both the
    /// captured acc and the element Float). Separate from the Int `jit_native_block_acc`.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_block_acc_float: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeProto>>,
    /// Per block-proto cache of whole-loop Float each-accumulator drivers.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_floateach_acc_loop: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeLoop>>,
    /// Int-element / Float-accumulator each-acc block (`ints.each { |x| t += x*1.5 }`):
    /// Int element param, Float captured accumulator. Distinct from both the Int and
    /// the Float-element each-acc blocks.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_block_acc_intelem_fa: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeProto>>,
    /// Per block-proto cache of the Int-element/Float-acc each-accumulator loop (int
    /// element reader, opaque Float-bits accumulator).
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_intelem_floateach_acc_loop: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeLoop>>,
    /// Per-(block proto, is_min) cache of whole-loop `min_by` / `max_by` drivers —
    /// a fold tracking the best key + its element (the value-mode block is the key
    /// function, shared with `jit_native_block`).
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_minmax_loop: crate::intern::FxHashMap<(usize, bool), Option<crate::jit_native::NativeLoop>>,
    /// Float variant of `jit_native_minmax_loop` (Float element + Float key).
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_floatminmax_loop: crate::intern::FxHashMap<(usize, bool), Option<crate::jit_native::NativeLoop>>,
    /// Int-element / Float-KEY min_by/max_by loop (`ints.min_by { |x| x*1.5 }`):
    /// Int element returned, Float key compared. Keyed by (block proto, is_min).
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_intelem_floatminmax_loop: crate::intern::FxHashMap<(usize, bool), Option<crate::jit_native::NativeLoop>>,
    /// Float PREDICATE block compilation (param Float, returns Bool via fcmp), for
    /// Float `count`/`select`/`reject`/`find`. Separate from Int `jit_native_block_pred`.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_block_pred_float: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeProto>>,
    /// Per block-proto caches of whole-loop Float count/find drivers + the
    /// (proto, keep)-keyed Float filter (select/reject) driver.
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_floatcount_loop: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeLoop>>,
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_floatfilter_loop: crate::intern::FxHashMap<(usize, bool), Option<crate::jit_native::NativeLoop>>,
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_floatfind_loop: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeLoop>>,
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native_on: bool,
    pub(crate) interner: Interner,
    pub(crate) classes: FxHashMap<SymId, Rc<Class>>,
    /// Bare constant assignments (`FOO = expr`), kept in a separate
    /// table from `classes` so `class Foo` and `Foo = 42` can coexist
    /// without collision. `Op::LoadConst` resolves classes first,
    /// then this table, then the `ENV` special-case — chosen for
    /// implementation simplicity, NOT to mirror CRuby (CRuby would
    /// emit "already initialized constant" and reassign). If you
    /// need to shadow a class with a constant, pick a different name.
    pub(crate) constants: FxHashMap<SymId, Value>,
    /// Definition location of each user-defined class/module/value
    /// constant, keyed by the same qualified-name `SymId` the
    /// `classes` / `constants` tables use. Recorded at
    /// `Op::DefClass` / `Op::DefModule` / `Op::StoreConst` with a
    /// FIRST-definition-wins rule for every shape. For class/module
    /// reopens that matches CRuby (reopens don't move the location);
    /// for value-constant REASSIGNMENT CRuby moves the location to
    /// the reassigning write (alongside the "already initialized
    /// constant" warning) while rubyrs keeps the first — a
    /// pre-existing, deliberate divergence (`remove_const` +
    /// redefine does re-stamp). Read by `Module#const_source_location`.
    /// `Rc<str>` filename is not a GC object, so no rooting.
    /// Snapshot/reset-managed like `constants` so embed resets don't
    /// leak user entries.
    ///
    /// Rc-pinning trade (deliberate): each `ConstLoc` pins the
    /// source-text version its define executed against, so an
    /// embedder that hot-re-`load`s the same path accumulates the
    /// old versions for as long as their constants stay stamped —
    /// the cost of keeping every already-stamped answer stable
    /// after a re-load overwrites `vm.sources`.
    ///
    /// Stores the RAW location ingredients (filename + captured
    /// source text + byte offset), NOT a resolved line: resolving a
    /// byte offset to a line via `error::line_col` scans the source
    /// from byte 0, so eager define-time stamping was
    /// O(defines × offset) — quadratic per file, ~65-70% of
    /// preamble-replay execution and the same tax again on every
    /// user file load. Definition-time stamping is now O(1);
    /// `Module#const_source_location` (the only value reader)
    /// resolves the line lazily per query against the captured
    /// source Rc, which pins the exact text the eager stamp would
    /// have scanned even if `vm.sources` is later overwritten by a
    /// re-`load` of the same path.
    pub(crate) const_source_locations: FxHashMap<SymId, ConstLoc>,
    /// Files already loaded via `require_relative` — keyed by
    /// canonical path. Suppresses re-loading on subsequent calls
    /// the same way CRuby's `$LOADED_FEATURES` does. The Set
    /// shape (no associated value) is intentional: rubyrs doesn't
    /// expose the list to script code yet, and the "true → false"
    /// return semantic only needs membership.
    ///
    /// Gated to non-wasi: `require` / `require_relative` short-
    /// circuit to a trap on wasm32-wasi (no file I/O), so the
    /// field would be dead code there and trip `-D dead_code`
    /// under `--no-default-features` (the only meaningful wasi
    /// build shape).
    #[cfg(not(target_os = "wasi"))]
    pub(crate) loaded_features: std::collections::HashSet<std::path::PathBuf>,
    /// Canon paths whose `require` body has SUCCESSFULLY COMPLETED
    /// (a strict subset of `loaded_features`, which also holds
    /// mid-load files for circular-require dedup). Lets `require`
    /// honor the dev-reload idiom `$LOADED_FEATURES.delete(path);
    /// require path` (sinatra/reloader, Rails) WITHOUT mistaking a
    /// mid-load circular re-require for a forced reload: only a
    /// COMPLETED feature that the user removed from the script-
    /// visible `$LOADED_FEATURES` array re-loads; a still-loading
    /// one (not yet here) keeps deduping.
    #[cfg(not(target_os = "wasi"))]
    pub(crate) completed_features: std::collections::HashSet<std::path::PathBuf>,
    /// Set of stdlib stub names (`uri`, `logger`, `json`, ...)
    /// that have been "loaded" via the lenient require stub.
    /// CRuby's `require` returns `true` on first load and
    /// `false` on every subsequent call for the same feature;
    /// rubyrs was returning `true` every time because the
    /// stub didn't track per-name state. Tracked separately
    /// from `loaded_features` (which keys on canonical
    /// `PathBuf`) because stubs have no path. Same wasi-gate
    /// for the same reason — `require` is a trap on wasm32-
    /// wasi so the field would be dead code.
    #[cfg(not(target_os = "wasi"))]
    pub(crate) loaded_stdlib_stubs: std::collections::HashSet<String>,
    /// Top-level pending autoload registry — `autoload :Foo, "path"`
    /// at toplevel records `(Foo, "path")` here. First reference to
    /// `Foo` via `Op::LoadConst` pops the entry and calls `require`;
    /// `autoload?(:Foo)` reads it without firing.
    ///
    /// Wasi-gated like `loaded_features`: the trigger needs
    /// `require`, which traps on wasm32-wasi (no file I/O).
    ///
    /// Phase 1 scope (issue #224): toplevel-only. Per-class autoloads
    /// (`Mod.autoload :Foo, "p"`) remain no-op stubs at the existing
    /// `Value::Class(_)` dispatch arms; Phase 2 will add a per-Class
    /// `autoloads` field and the LoadConstChain / resolve_const_path
    /// trigger points.
    #[cfg(not(target_os = "wasi"))]
    pub(crate) autoloads_toplevel: HashMap<SymId, String>,
    /// Per-class (scoped) pending autoload registry — Phase 2 of
    /// issue #224. `Mod.autoload :Foo, "path"` (or a bare
    /// `autoload :Foo, "path"` inside a `module Mod` body, where
    /// self is the Class) records the entry here keyed by the
    /// QUALIFIED-name SymId (`intern("Mod::Foo")`), exactly
    /// parallel to how named-class constants live in
    /// `self.constants`. First reference to `Mod::Foo` via
    /// `resolve_const_path` that would otherwise miss pops the
    /// entry and `require`s the path, then re-resolves;
    /// `Mod.autoload?(:Foo)` reads it without firing.
    ///
    /// Rack 3 / Sinatra register 40+ of these
    /// (`autoload :Response, 'rack/response'`, …) at module-load
    /// time; without the trigger every `Rack::Response` /
    /// `Rack::Builder` / `Rack::Utils` reference NameErrors.
    ///
    /// Wasi-gated like `autoloads_toplevel`: the trigger needs
    /// `require`, which traps on wasm32-wasi (no file I/O).
    #[cfg(not(target_os = "wasi"))]
    pub(crate) autoloads_scoped: HashMap<SymId, String>,
    /// Constant keys (full qualified-name SymIds) whose autoload was
    /// FIRED/consumed but whose file did NOT define the constant. CRuby
    /// leaves such a const in a removable "undef-after-autoload" slot:
    /// `autoload?`/`const_defined?` report nil/false, yet `remove_const`
    /// SUCCEEDS (no "not defined" NameError). zeitwerk's on_file_autoloaded
    /// relies on this — it `remove_const`s then raises Zeitwerk::NameError.
    /// Cleared when the constant is later actually defined.
    pub(crate) consumed_autoloads: std::collections::HashSet<SymId>,
    /// Qualified const keys (`"M::X"` SymIds) marked `private_constant`.
    /// CRuby raises "private constant M::X referenced" on EXPLICIT `M::X`
    /// access (even from inside M), while bare/lexical reads and `const_get`
    /// still work. Enforced in the qualified Op::LoadConst path only.
    pub(crate) private_consts: std::collections::HashSet<SymId>,
    /// Reverse map: canonicalized autoload-target path -> the const keys whose
    /// autoload points there. CRuby `require`ing a file SATISFIES (consumes)
    /// any autoload registered for it (`autoload?` returns nil afterward).
    /// Populated at autoload-registration time (one canonicalize there, none in
    /// the require hot path); consulted O(1) when a require completes.
    // Read/written only by autoload registration + require completion, both
    // wasi-gated (no `require` on wasm32-wasi), so it's dead on that target.
    #[cfg_attr(target_os = "wasi", allow(dead_code))]
    pub(crate) autoload_paths: std::collections::HashMap<std::path::PathBuf, Vec<SymId>>,
    /// Per-call-site inline-cache counter. Each compiled `Op::Call`
    /// gets a unique u16 slot id; the Vm side allocates
    /// `call_caches[id]` lazily. Lives on the Vm so kernel
    /// builtins (e.g. `require_relative`) that compile new Ruby
    /// source at runtime can advance the counters without
    /// round-tripping through Runtime. `.call` = method-call sites
    /// (`call_caches`), `.ivar` = ivar-access sites (`ivar_caches`,
    /// ADR 0035 Ph4/5) — separate id spaces, see `CidGen`.
    pub(crate) cache_counter: crate::compiler::CidGen,
    /// User-defined global variables (`$foo = 1; puts $foo`).
    /// Keyed by SymId of the name including the leading `$`.
    /// Reads of unknown globals return Nil (matches CRuby's
    /// lenient "uninitialized global variable" silent default).
    /// Special globals — `$$` (process pid), `$0` (script name),
    /// regex backrefs `$~` / `$1`–`$9`, separators `$,` / `$;` —
    /// are not stored here; `Op::LoadGlobal` intercepts a known
    /// set and returns the computed value. Plain user globals
    /// fall through to this table.
    pub(crate) globals: FxHashMap<SymId, Value>,
    pub(crate) toplevel_methods: FxHashMap<SymId, Rc<Method>>,
    /// The top-level `main` object — the `self` of a script's (and a
    /// required file's / bare `eval`'s) top level, matching CRuby where
    /// `self` there is a singleton Object (not nil). Created lazily on
    /// first top-level frame AFTER `Object` exists (so the preamble,
    /// which runs before `Object` is defined, keeps `self = nil`), then
    /// reused so `self.extend Module` accumulates on the one main across
    /// evals. GC-rooted in the mark phase. `None` until materialised.
    pub(crate) main_obj: Option<ObjId>,
    /// Toplevel `@@foo` fallback. CRuby raises RuntimeError on
    /// class-variable use outside a class body; rubyrs takes the
    /// lenient route consistent with our ivar / global handling.
    /// Inside a class body / instance method / class method, the
    /// surrounding `Rc<Class>` owns the cvar; this table catches
    /// the toplevel-only `@@x` writes scripts occasionally use
    /// for cache-like state at file scope.
    pub(crate) toplevel_cvars: HashMap<SymId, Value>,
    /// Per-`@@cvar`-site owner caches, dense by the
    /// `Op::LoadCvar`/`StoreCvar` cid (`CidGen::cvar` space).
    /// Validated against `cvar_gen`; see `CvarSiteCache`.
    pub(crate) cvar_caches: Vec<crate::vm::lookup::CvarSiteCache>,
    /// Generation for `cvar_caches`: bumped whenever cvar OWNERSHIP
    /// resolution can change — a `@@name` created on a class that
    /// didn't own it (StoreCvar / class_variable_set create path),
    /// a superclass rewire (reopen-with-parent), and the
    /// snapshot/reset restore paths (which rewrite `class_vars`
    /// tables wholesale). Value overwrites on the existing owner
    /// do NOT bump (the owner is unchanged — that's what makes the
    /// cache profitable for the `@@x ||= …` read/write pattern).
    pub(crate) cvar_gen: u32,
    /// Per-`super`-site resolved-method caches, dense by the
    /// `Op::Super`/`ApplySuper`/`ApplySuperBlock` cid
    /// (`CidGen::sup` space). Validated against `method_gen`; see
    /// `SuperSiteCache`.
    pub(crate) super_caches: Vec<crate::vm::lookup::SuperSiteCache>,
    /// Heap-allocated `$LOAD_PATH` / `$:` Array. Lazily
    /// initialised on first read so cold-eval scripts that
    /// never touch it pay zero startup cost. Scripts can
    /// `$LOAD_PATH.unshift(dir)` — mutations on this ObjId
    /// land in the same heap Array the require dispatcher
    /// later reads from. GC-rooted in `maybe_gc`.
    pub(crate) load_path: Option<ObjId>,
    /// `$LOADED_FEATURES` / `$"` — the Array of canonical paths of
    /// files already loaded via `require` / `load`. Lazily
    /// materialised (same as `load_path`); each successful
    /// `compile_and_run_source` pushes the canonical path. Exposed so
    /// script code can read it (`$LOADED_FEATURES.last` to find the
    /// just-loaded file — zeitwerk's `Kernel#require` wrapper does
    /// this) and mutate it (`reject!` during reloading/unload). The
    /// internal `loaded_features` Set remains the require-dedup
    /// authority; this Array is the script-visible view. GC-rooted in
    /// `maybe_gc`.
    pub(crate) loaded_features_list: Option<ObjId>,
    pub(crate) host_fns: HashMap<SymId, HostFnSlot>,
    /// C-ext singleton-method dispatch table. Indexed by
    /// `(class joined name, method SymId)`. Populated by
    /// `Vm::cext_require` whenever a C ext calls
    /// `rb_define_singleton_method`; consulted by `do_call` when
    /// the receiver is `Value::Class(c)`.
    #[cfg(feature = "cext")]
    pub(crate) cext_class_methods: HashMap<String, HashMap<SymId, Rc<HostFn>>>,
    /// L3-C: instance-method dispatch table for cext-registered
    /// methods (`rb_define_method`). Mirrors `cext_class_methods`'s
    /// shape but consulted when the receiver is `Value::Object(id)`
    /// whose class joined-name matches. Stores raw registration
    /// data instead of a HostFn closure because the receiver isn't
    /// known at registration time; the dispatch site assembles
    /// `cext_dispatch(..., CextSelfHandle::Object(recv))` inline.
    #[cfg(all(feature = "cext", not(target_os = "wasi")))]
    pub(crate) cext_instance_methods: HashMap<String, HashMap<SymId, crate::vm::cext::CextMethodReg>>,
    pub(crate) class_stack: Vec<Rc<Class>>,
    /// Per-class-body visibility mode, parallel to `class_stack`.
    /// Pushed `Public` on `Op::DefClass` and popped when the class
    /// body returns. Read by `Op::DefMethod` to stamp new methods
    /// with the current visibility, and mutated by the no-arg
    /// `private` / `protected` / `public` calls.
    pub(crate) class_visibility_stack: Vec<Visibility>,
    /// Parallel to `class_stack` / `class_visibility_stack` —
    /// `true` once a class body has invoked the bare-form
    /// `module_function` (no args). Subsequent `Op::DefMethod`
    /// inside the same body then dual-installs: the instance
    /// method gets stamped Private (via the visibility stack
    /// flip module_function already performs), AND a public
    /// clone goes to the class's `singleton_methods` so
    /// `Module.method_name(...)` resolves at call time. Cleared
    /// (popped) when the class body returns. Symbol/string-arg
    /// `module_function(:foo, :bar)` is unaffected — it
    /// retroactively installs already-defined methods on the
    /// singleton (see the dedicated arm in vm/dispatch.rs); the
    /// flag here governs the FORWARD-LOOKING bare-form contract.
    pub(crate) module_function_active_stack: Vec<bool>,
    /// Compiled-regex cache. Keyed by the interned source-string
    /// SymId; first `LoadRegex` for a given pattern compiles and
    /// caches, subsequent loads return the same Rc. Cfg-gated on
    /// the `regex` feature (ADR 0017 Rule 3) — disappears with
    /// `--no-default-features`.
    /// Keyed by `(source SymId, Ruby flag bitmask)` so the same
    /// source text with different flags (`/foo/` vs `/foo/i`)
    /// compiles to distinct cached regexps rather than colliding.
    #[cfg(feature = "regex")]
    pub(crate) regex_cache: HashMap<(SymId, u8), Rc<crate::regex_engine::CompiledRegex>>,
    /// Parsed-BigInt cache for `Op::LoadBigInt`. Keyed by the
    /// interned decimal-string SymId; first load decodes via
    /// `BigInt::from_str`, subsequent loads return the cached
    /// `Rc<BigInt>` (a fresh `HeapObj::BigInt(b.clone())` is
    /// allocated per load so the heap-side identity stays
    /// per-Value, but the parse work is amortised).
    #[cfg(feature = "bignum")]
    pub(crate) bigint_lit_cache: HashMap<SymId, Rc<num_bigint::BigInt>>,
    /// Last successful regex match — populated by `=~`,
    /// `String#match`, and `Regexp#===` when they hit, cleared
    /// when they miss. Source of truth for `$~` and `$1`..`$N`
    /// (NumberedReferenceReadNode — any positive index, matching
    /// CRuby; `$10`+ are valid too) reads in `LoadGlobal`. Owned
    /// strings rather than
    /// a heap ObjId so we don't have to wire a GC-walk root for
    /// what is conceptually a fast side-channel; `$~` materialises
    /// a fresh MatchData instance on demand. Cfg-gated on `regex`
    /// — without the feature there are no successful matches to
    /// record.
    #[cfg(feature = "regex")]
    pub(crate) last_match: Option<LastMatch>,
    /// Lazily-built ENV Hash, shared across every `ENV`
    /// reference. Set on first `LoadConst("ENV")` and reused
    /// thereafter so script code observes a single mutable
    /// snapshot of the env map the host provided via
    /// `Config::env`. With `Config::env = None`, the lazy build
    /// produces an empty Hash.
    pub(crate) env_hash: Option<ObjId>,
    /// Host-injected ENV map (from `Config::env`). `None` means
    /// "expose an empty ENV Hash" — the script's `ENV[k]` reads
    /// see no host process env vars. ADR 0017 Rule 1+2 closure
    /// for the previous `std::env::vars()` deviation. CLI binary
    /// fills this from `std::env::vars()` to preserve `rubyrs
    /// script.rb` ergonomics.
    pub(crate) env_override: Option<HashMap<String, String>>,
    /// Host-injected PID exposed to scripts via `$$` (from
    /// `Config::pid`). `None` means `$$` returns `0` (sentinel).
    /// ADR 0017 Rule 1 closure for the previous
    /// `std::process::id()` deviation.
    pub(crate) pid: Option<i64>,
    /// Host-injected wall-clock source for `Time.now`. `None`
    /// means `__time_now_raw` raises (deterministic Tier 1
    /// default); CLI binary fills this from
    /// `std::time::SystemTime::now()`. ADR 0017 Rule 1 closure
    /// for the previous "no Time class at all" status.
    pub(crate) time_now: Option<std::sync::Arc<dyn Fn() -> (i64, u32) + Send + Sync>>,
    /// Host-injected wall-clock sleep for `Kernel#sleep`.
    /// `None` means `sleep` raises (deterministic Tier 1
    /// default); CLI binary fills this with
    /// `std::thread::sleep`. See `Config::sleep_for` for
    /// rationale + ADR 0017 Rule 1 closure pattern.
    // clippy::type_complexity — the Fn signature is the
    // contract the embed host implements (deadline + interrupt
    // flag → elapsed Duration); extracting it to a `type` alias
    // here would just hide the contract one level. The mirror
    // `Config::sleep_for` field uses the same shape.
    #[allow(clippy::type_complexity)]
    pub(crate) sleep_for: Option<std::sync::Arc<
        dyn Fn(Option<std::time::Duration>, &std::sync::atomic::AtomicBool) -> std::time::Duration
            + Send + Sync,
    >>,
    /// Host-injected immediate-exit closure for `Kernel#exit!`.
    /// `None` means `exit!` raises (Tier 1 deterministic default).
    /// CLI binary fills with `std::process::exit`. See
    /// `Config::process_exit` for rationale.
    pub(crate) process_exit: Option<std::sync::Arc<dyn Fn(i32) + Send + Sync>>,
    /// ADR 0025 Phase 1: process-wide SIGINT-arrived flag,
    /// shared between every Runtime opting in via
    /// `Config::install_signal_handler`. The signal handler's
    /// only action is `AtomicBool::store(true, SeqCst)` —
    /// async-signal-safe by construction (single relaxed-or-
    /// stronger atomic instruction, no allocation, no
    /// locking).
    ///
    /// Phase 2 will add the `dispatch_until` top-of-loop
    /// consumer that translates a set flag into a Ruby-level
    /// `Interrupt` trap.
    ///
    /// Embedders with `install_signal_handler: false` still
    /// hold an Arc to AN AtomicBool — either the shared
    /// process-wide one (if another Runtime opted in) or a
    /// dedicated local one (nothing ever writes to it). The
    /// safe-point read is the same atomic load either way; the
    /// install gate is purely about whether the handler is
    /// registered.
    pub(crate) interrupt_pending: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// ADR 0025 Phase 2 + Risk #9 — "must-complete cleanup
    /// window" counter. While > 0, the dispatch_until
    /// safe-point check leaves `interrupt_pending` set but
    /// doesn't deliver. Used by close paths (FiberResponseBody
    /// Drop runs `body.close` through dispatch_until; without
    /// this guard a concurrent SIGINT would abort close
    /// mid-flight — exactly ADR 0023 Risk #1's ensure-leak
    /// shape). Counter, not bool, so nested suppress windows
    /// work. RAII-managed via `SuppressInterruptGuard` (Phase
    /// 4 wires the wrap on FiberResponseBody::drop).
    ///
    /// Vm-wide; NOT stashed in FiberSnapshot. Close paths trap
    /// on `Fiber.yield` (FiberError, mirroring `cext_depth`'s
    /// existing guard) so no Fiber-suspend can happen inside
    /// a suppress window.
    pub(crate) suppress_interrupt: u32,
    /// ADR 0025 Phase 4: user-installed signal handlers
    /// keyed by Unix signal number. Default state for an
    /// un-installed signal is `SignalHandlerState::Default`
    /// (translates to a Ruby `Interrupt` raise at safe
    /// point). Populated via `Signal.trap("INT") { ... }`;
    /// consumed by the safe-point check (Phase 4b) which
    /// switches between raise / no-op / re-entrant block
    /// invocation based on the state.
    pub(crate) signal_traps: std::collections::HashMap<i32, SignalHandlerState>,
    /// ADR 0024 Phase A Risk #1: bounds Rust-stack growth
    /// from recursive `Op::Yield` (synchronous wrapper
    /// re-enters `dispatch_until` per yield-site chain).
    /// Incremented by `YieldDepthGuard::enter`, decremented
    /// by its `Drop` — panic-safe. Cap (`Config::max_yield_recursion`)
    /// trips `ResourceExhausted` to keep adversarial scripts
    /// from blowing the Rust stack via deep yield nesting.
    /// Vm-wide; NOT stashed in FiberSnapshot — same as
    /// `cext_depth`.
    #[allow(dead_code)] // wired in Phase A.1
    pub(crate) yield_recursion_depth: u32,
    /// Cap on `yield_recursion_depth`. `None` = unlimited
    /// (default). Mirrors `Config::max_live_fibers` shape.
    /// Default value when `Config::max_yield_recursion: None`:
    /// 256 (defensive — well above typical recursive-yield
    /// depths, below where the Rust stack runs out under
    /// default 8MB).
    pub(crate) max_yield_recursion: Option<u32>,
    /// ADR 0025 Phase 4c: `Kernel#at_exit { ... }` handlers
    /// registered for end-of-eval execution. Drained LIFO by
    /// the public `Runtime::eval` wrapper after `eval_inner`
    /// returns — runs on both happy-path (Ok result) and
    /// SystemExit / other Trap paths. Skipped only by
    /// `Kernel#exit!` (which calls `Config::process_exit`
    /// and never returns to Rust).
    ///
    /// CRuby model: at_exit handlers fire at process end.
    /// Embed model adaptation: each `Runtime::eval()` call is
    /// the "process" — handlers drain at its end. Documented
    /// in `Kernel#at_exit`'s docstring.
    pub(crate) at_exit_handlers: Vec<crate::value::ObjId>,
    /// The exception VALUE behind the most recent
    /// `RubyError::Uncaught` trap (the trap itself only carries
    /// class_name + message strings). Consumer: the fork-child
    /// exit path reads `@status` off an uncaught SystemExit.
    /// GC-rooted by the root gather.
    pub(crate) last_uncaught_exception: Option<Value>,
    /// Same-process Marshal round-trip registry (ADR 0017-honest
    /// subset): `Marshal.dump` stashes the object here and returns
    /// a YAML-comment token naming the slot; `Marshal.load` of that
    /// exact token returns the SAME object (shallow — documented
    /// divergence from CRuby's deep copy). Any other input —
    /// including real marshal bytes and tokens from another process
    /// — raises TypeError, so cross-process consumers (Jekyll's
    /// regenerator) fall through their rescue chains unchanged.
    /// Rooted by the GC gather walk; capped (see MARSHAL_REGISTRY_CAP)
    /// so a dump loop can't pin unbounded memory.
    pub(crate) marshal_registry: Vec<Value>,
    /// Creation counter feeding `Class.anon_serial` (see
    /// value::class_display_name) — deterministic stand-in for
    /// CRuby's address digits in `#<Class:0x...>`.
    pub(crate) anon_class_counter: u32,
    /// True once ANY Hash gained a per-instance eigenclass —
    /// the dispatch gate that keeps plain-Hash traffic free of
    /// the singleton probe (set-once, never cleared; mirrors
    /// `any_undefs`).
    pub(crate) any_hash_singletons: bool,
    /// String per-instance eigenclasses (`(+"s").stub :to_s, ...` /
    /// `def s.foo`). `Value::Str` is a bare `Rc<RStr>` with no room
    /// for an eigenclass pointer (strings are the hottest heap
    /// shape), so the eigenclass lives in this side-table keyed on
    /// the Rc's pointer identity. The map holds a strong Rc to the
    /// RStr so a freed-and-reused allocation can't alias an old
    /// entry (string singletons are test-harness-rare; the leak is
    /// bounded by their count). Walked by the GC root gather (the
    /// eigenclass's Methods capture GC objects). `any_str_singletons`
    /// is the set-once dispatch gate, same as `any_hash_singletons`.
    pub(crate) str_singletons: crate::intern::FxHashMap<usize, (std::rc::Rc<crate::value::RStr>, Rc<Class>)>,
    pub(crate) any_str_singletons: bool,
    /// Instance variables on String VALUES — CRuby lets you set ivars
    /// on a String (`str.instance_variable_set(:@x, 1)`); `RStr` has no
    /// ivar slot, so they live in this side-table keyed by the Rc's
    /// pointer identity (same shape/leak-tradeoff as `str_singletons`:
    /// the strong Rc keeps the string alive so a freed-and-reused
    /// allocation can't alias a stale entry). The values are GC objects,
    /// so the root gather walks them. `any_str_ivars` is the set-once
    /// dispatch gate. Motivating case: serbea's `String#html_safe` does
    /// `dup.tap { _1.instance_variable_set(:@html_safe, true) }`
    /// (Bridgetown's ERB render path).
    pub(crate) str_ivars: crate::intern::FxHashMap<usize, (std::rc::Rc<crate::value::RStr>, crate::intern::FxHashMap<crate::intern::SymId, Value>)>,
    pub(crate) any_str_ivars: bool,
    /// Per-instance eigenclasses for HEAP objects keyed by their
    /// `ObjId.0` — currently `Array` and `Proc`/`Block`, the two
    /// `define_singleton_method` / `def obj.x` targets the String
    /// table's pattern didn't cover (Hash carries its eigenclass on
    /// the heap struct). The stored `Value` roots the object so the
    /// id can't be swept + reused under the key (the str table holds
    /// the `Rc<RStr>` for the same reason). `any_heap_singletons` is
    /// the set-once dispatch gate. rack's Deflater/Lock define
    /// `:close` on an Array body; ContentLength defines `:each` on a
    /// Proc body.
    pub(crate) heap_singletons: crate::intern::FxHashMap<usize, (crate::value::Value, Rc<Class>)>,
    pub(crate) any_heap_singletons: bool,
    /// `Kernel#binding` local snapshots, keyed by the Binding
    /// instance's `ObjId`: the (name, value) of each NAMED local in
    /// the capturing frame. `eval(src, binding)` re-seeds them as
    /// same-order params so the eval'd source resolves them (rack's
    /// ShowExceptions/ShowStatus ERB templates read the calling
    /// method's locals). A snapshot (read-only): writes in the eval'd
    /// source don't propagate back (CRuby's binding is live; ERB
    /// doesn't rely on write-back). GC roots the captured Values.
    pub(crate) binding_locals: crate::intern::FxHashMap<usize, Vec<(String, crate::value::Value)>>,
    /// `Encoding.default_external` (E3): the tag File.read stamps
    /// when no `encoding:` argument is given. CRuby's process-wide
    /// default; ours starts at UTF-8 and is set through the
    /// preamble's `Encoding.default_external=` →
    /// `__rubyrs_set_default_external`.
    pub(crate) default_external: crate::value::EncodingTag,
    /// `Encoding.default_internal` (E3): when Some, a tag-less or
    /// single-name File.read TRANSCODES external→internal instead
    /// of just tagging (CRuby's default is nil = no conversion).
    pub(crate) default_internal: Option<crate::value::EncodingTag>,
    /// Refinements (`refine` / `using`). Tier-1: activation is GLOBAL
    /// from the `using` point on, not lexically scoped per file/module
    /// like CRuby — equivalent for the common single-file case (see
    /// SUBSET.md). All three are empty until a script calls `refine`, so
    /// programs that never use refinements pay nothing.
    ///
    /// `module_refinements`: keyed by `Rc::as_ptr(M) as usize` (the
    /// defining module), the list of `(target_class, refinement_holder)`
    /// pairs `refine` recorded; `using M` reads it to activate.
    pub(crate) module_refinements: crate::intern::FxHashMap<usize, RefinementList>,
    /// Reverse link from a refinement holder (the empty anon module `refine
    /// Target do … end` creates) to its `Target`. Keyed by
    /// `Rc::as_ptr(holder) as usize`. Lets `alias`/`alias_method` inside the
    /// refine block resolve the source method — including a primitive like
    /// `Array#sum` — from `Target` rather than the empty holder (the AS
    /// `refine Array do alias :orig_sum :sum end` shape).
    pub(crate) refinement_targets:
        crate::intern::FxHashMap<usize, std::rc::Rc<crate::value::Class>>,
    /// `(target_class_name, method_name)` → the active refined method.
    pub(crate) active_refinements:
        crate::intern::FxHashMap<(SymId, SymId), std::rc::Rc<crate::value::Method>>,
    /// Method names with ANY active refinement — a cheap dispatch gate so
    /// non-refined calls skip the `active_refinements` lookup entirely.
    pub(crate) refined_method_names: crate::intern::FxHashSet<SymId>,
    pub(crate) stack: Vec<Value>,
    pub(crate) frames: Vec<Frame>,
    /// Recycled `FrameAux` boxes (cleared, capacities kept warm) so a
    /// begin/rescue-bearing method doesn't malloc a fresh box + Vec
    /// backings on EVERY call (rubocop's `with_cop_error_handling`
    /// enters a begin 25.6k times per file walk). `top_aux_mut` pops
    /// from here; the frame-pop sites push back via
    /// `recycle_frame_aux`. Pooled boxes hold NO `Value`s (cleared at
    /// recycle time), so the GC never needs to walk this.
    #[allow(clippy::vec_box)] // pools the Box ALLOCATIONS: entries move into `Frame::aux: Option<Box<FrameAux>>` without re-boxing
    pub(crate) frame_aux_pool: Vec<Box<FrameAux>>,
    pub(crate) heap: Heap,
    /// Native-code holding pen for heap values across GC points; see ADR 0005.
    pub(crate) pinned: Vec<Value>,
    pub(crate) stdout: Box<dyn std::io::Write>,
    /// Tier-1 2c: separate channel for `warn` / `abort` no-args /
    /// future `STDERR.puts` / `$stderr.write`. Defaults to
    /// `std::io::sink()` so embedders that don't care don't see
    /// any output — same defensive default as `stdout`. The CLI
    /// binary wires this to `std::io::stderr()`; tests can wire a
    /// capturing buffer.
    pub(crate) stderr: Box<dyn std::io::Write>,
    pub(crate) stress_gc: bool,
    /// Mirror of `Config::allow_filesystem_io`. Set by
    /// `apply_config` (and the preamble snapshot path); read by
    /// every script-callable FS-touching site via
    /// `Vm::check_filesystem_io_allowed`. `false` (the default)
    /// makes File.*/require/__dir__ trap; `true` lets them
    /// through.
    pub(crate) allow_filesystem_io: bool,
    /// Mirror of `Config::allow_process_spawn` — gates
    /// Kernel#system / backtick subprocess execution.
    pub(crate) allow_process_spawn: bool,
    /// Mirror of `Config::allowed_paths`. When `Some(prefixes)`,
    /// each FS op's resolved path is checked against the
    /// prefixes before proceeding (see
    /// `Vm::check_path_in_allowlist`). Entries are canonicalized
    /// once by `apply_config` so the per-op check can do a pure
    /// lexical resolve + `starts_with` without further syscalls.
    /// `None` (default) means no narrowing on top of the bool.
    pub(crate) allowed_paths: Option<Vec<std::path::PathBuf>>,
    /// Mirror of `Config::sqlite_allow_paths` — per-battery
    /// sandbox per ADR 0027 §7. Checked by
    /// `sqlite::check_path_allowed` at `Database.new` time.
    #[cfg(feature = "_sqlite")]
    pub(crate) sqlite_allow_paths: Option<Vec<std::path::PathBuf>>,
    /// Mirror of `Config::sqlite_max_result_bytes` per ADR 0027
    /// §7b. Heap-cap on `query` result-set materialisation; the
    /// battery accumulates row bytes and traps
    /// `SQLite3::TooBigException` when the running total exceeds.
    #[cfg(feature = "_sqlite")]
    pub(crate) sqlite_max_result_bytes: Option<usize>,
    /// Mirror of `Config::allow_network_io` — `_socket` battery
    /// master gate (ADR 0028 §2). Checked by `socket::check_connect_allowed`.
    #[cfg(feature = "_socket")]
    pub(crate) allow_network_io: bool,
    /// Mirror of `Config::socket_allow_hosts` — `_socket` host
    /// allowlist (ADR 0028 §2 / ADR 0019 Rule 4).
    #[cfg(feature = "_socket")]
    pub(crate) socket_allow_hosts: Option<Vec<String>>,
    /// Mirror of `Config::socket_max_read_bytes` — `_socket`
    /// per-socket read cap (ADR 0028 §2, class-`f`).
    #[cfg(feature = "_socket")]
    pub(crate) socket_max_read_bytes: Option<usize>,
    /// Per-eval working counter; `Some(0)` means exhausted, `None`
    /// means unlimited. Re-anchored at each `Runtime::eval` entry
    /// from `Runtime::fuel_budget` (which `apply_config` writes
    /// from `Config::fuel`); decremented per op by `check_fuel`.
    pub(crate) fuel: Option<u64>,
    /// Maximum simultaneously-live frames. `frames.push()` checks this
    /// against `frames.len()` before pushing. Default `None` is unlimited.
    pub(crate) max_frames: Option<usize>,
    /// Embedder-tunable cap on re-entrant `dispatch_until` depth
    /// (block-call recursion through `Object#then` / `#tap` /
    /// `Proc#call` / `yield` / native iter drivers). Layered ON TOP
    /// of the always-on `DEFAULT_MAX_DISPATCH_DEPTH` SystemStackError
    /// cap inside `check_frames`: when `Some(n)`, trips
    /// `ResourceExhausted` (NOT SystemStackError) at `n` so untrusted
    /// scripts can't swallow the trap with bare `rescue`. Set this
    /// LOWER than the always-on default (500) for sandboxed code on
    /// tighter stacks (e.g. 2 MB worker threads). Default `None` =
    /// only the always-on cap applies. Mirrors `max_frames`'s shape.
    pub(crate) max_dispatch_depth: Option<usize>,
    /// Absolute wall-clock instant past which `eval` traps with
    /// `ResourceExhausted("wall-clock deadline exceeded")`. `None`
    /// means unlimited. Computed at `Runtime::eval` entry from the
    /// `Config::deadline` duration. Checked every 1024 ops (cheap
    /// enough that the syscall amortises out).
    pub(crate) deadline_at: Option<std::time::Instant>,
    /// Lightweight counter incremented per op so deadline checks
    /// only call `Instant::now()` periodically. Wraps; we only
    /// inspect the low bits.
    pub(crate) op_counter: u32,
    /// Cap on distinct interned symbols (P2-14b). `None` means
    /// unlimited. Checked at runtime intern sites (`to_sym`) before
    /// the actual `intern()` call; compile-time intern is not
    /// capped because it's already bounded by source size.
    pub(crate) max_symbols: Option<usize>,
    /// Per-value byte cap (P2-14c). Defends against single
    /// values that hog memory (`"a" * 10_000_000`, `arr <<` in
    /// a tight loop). Checked at mutation sites; see
    /// `Config::max_value_bytes` for the model.
    pub(crate) max_value_bytes: Option<usize>,
    /// Per-call-site monomorphic inline cache for method dispatch on
    /// `Value::Object`. One slot per `Op::Call(...,cache_id)` /
    /// `Op::CallNoRecv` / `Op::CallBlock` / `Op::CallNoRecvBlock` site.
    /// Each entry remembers the (class identity, gen-at-time-of-cache,
    /// resolved Method) of the last successful lookup at that site.
    ///
    /// Lookups compare against the receiver's class pointer AND the
    /// current `method_gen`. Any `Op::DefMethod` bumps `method_gen`,
    /// which effectively invalidates every cache entry — re-fill is
    /// lazy on the next call at each site.
    pub(crate) call_caches: Vec<CallCache>,
    /// Per-ivar-site inline caches (ADR 0035 Ph4/5), dense by the
    /// `Op::LoadIvar`/`StoreIvar`/`IncIvar*` cid (`CidGen::ivar`
    /// space). See `IvarSiteCache` for the no-invalidation contract.
    pub(crate) ivar_caches: Vec<crate::vm::lookup::IvarSiteCache>,
    pub(crate) method_gen: u32,
    /// Inline constant caches. `Op::LoadConst` resolution depends only
    /// on the GLOBAL classes/constants tables, so one entry per SymId
    /// is valid program-wide; `Op::LoadConstChain` resolution is static
    /// per (proto, chain) — the lexical scope is compiled into the
    /// chain — so that pair keys it. Entries carry the `const_gen` they
    /// were filled at; ANY mutation that can change constant resolution
    /// (class/module definition, const assignment / const_set,
    /// name_anon_class re-homing, include/prepend — which alter the
    /// cref-ancestor walk) bumps `const_gen`, turning every entry stale
    /// (refilled lazily on the next read). Mirrors the method-IC
    /// `call_caches`/`method_gen` design.
    ///
    /// GC: a FRESH entry's Value is by construction still present in
    /// the canonical tables (nothing mutated since the fill), so it
    /// stays rooted through them; STALE entries are never dereferenced
    /// (gen check) — the caches therefore don't need to be GC roots.
    pub(crate) const_cache_flat: FxHashMap<SymId, (Value, u32)>,
    pub(crate) const_cache_chain: FxHashMap<(u32, u32), (Value, u32)>,
    pub(crate) const_gen: u32,
    pub(crate) sym_length: SymId,
    pub(crate) sym_size: SymId,
    pub(crate) sym_to_s: SymId,
    pub(crate) sym_inspect: SymId,
    /// Method names the `X.class_method` fast path must NOT take
    /// (`try_invoke_class_singleton_cached`): every name-keyed arm in
    /// `do_call` that can intercept a `Value::Class` receiver BEFORE
    /// the user-singleton lookup at the canonical Class-recv arm. For
    /// any name outside this set, the pre-arm chain provably falls
    /// through to that lookup, so resolving it early via the inline
    /// cache is semantics-identical. Over-inclusion is safe (those
    /// names just keep the slow path); UNDER-inclusion would let a
    /// user `def self.send`-style name bypass an intercepting arm —
    /// when adding a new Class-recv name arm to `do_call`, add the
    /// name here.
    pub(crate) class_singleton_deny: crate::intern::FxHashSet<SymId>,
    /// P5b name-keyed probe filter (see the builder in `Vm::new` for
    /// the maintenance contract): dense bitset by `SymId` index — bit
    /// set ⇔ some name-keyed pre-cascade bucket in `do_call` can serve
    /// this name. Consulted once per `do_call` via
    /// `probe_name_may_serve`; names interned after `Vm::new` (user
    /// method names) read `false` through the `get` bounds fallback.
    pub(crate) probe_name_mask: Vec<u64>,
    /// Pre-interned `$!` — read/written on every `begin/rescue` entry &
    /// exit (and `return` out of a rescue body) for the dynamically
    /// scoped errinfo, hot paths in exception-heavy code like Liquid
    /// rendering. Cached so those sites skip re-interning the literal.
    pub(crate) sym_bang: SymId,
    /// Pre-interned `[]` / `[]=` for the collection-index fast path
    /// (`try_fast_index`, vm/dispatch.rs).
    pub(crate) sym_index_op: SymId,
    pub(crate) sym_index_set_op: SymId,
    /// Pre-interned `call` for the proc/lambda-invocation fast path in
    /// do_call (skips the primitive-dispatch cascade for `p.call(...)`).
    pub(crate) sym_call: SymId,
    /// Cached current working directory (the `getcwd` result). `Dir.pwd`
    /// / `File.expand_path(rel)` resolve the cwd on every call; on macOS
    /// `getcwd` opens + walks the filesystem, which dominated a Sinatra
    /// request (Rack expand_path's per request). The cwd only changes via
    /// `Dir.chdir`, so cache it and invalidate there. `None` = unresolved.
    pub(crate) cwd_cache: Option<String>,
    /// Pre-interned `new` / `initialize` for the class-intrinsic `new`
    /// dispatch (was re-interned per `Object.new` — hot in OOP code).
    pub(crate) sym_new: SymId,
    pub(crate) sym_initialize: SymId,
    /// Pre-interned Hash key-probe names (`key?` / `has_key?` /
    /// `include?` / `member?` — one canonical hash.rs arm, four
    /// spellings) for the same fast path. Liquid's `Drop#invokable?`
    /// probes a Set (whose `include?` is the vendored interpreted
    /// wrapper around `@hash.include?`) on EVERY drop attribute
    /// access, and Jekyll data hashes get `key?`-probed throughout
    /// read/merge — both measured ~3.3k instructions through full
    /// dispatch vs ~1.4k in CRuby.
    pub(crate) sym_key_q: SymId,
    pub(crate) sym_has_key_q: SymId,
    pub(crate) sym_include_q: SymId,
    pub(crate) sym_member_q: SymId,
    /// Pre-interned `frozen?` for the zero-arg primitive fast path
    /// (Jekyll's `Utils.duplicate_frozen_values` probes it on every
    /// value of every document data hash, 4x per doc).
    pub(crate) sym_frozen_q: SymId,
    /// Pre-interned `nil?` / `empty?` for the zero-arg primitive fast
    /// path (PathManager.join-style guards probe both per call).
    pub(crate) sym_nil_q: SymId,
    pub(crate) sym_empty_q: SymId,
    /// Pre-interned `===` for the case-equality fast path (RuboCop's
    /// NodePattern matchers fire `SYM === node` / `Mod === node`
    /// millions of times per cop walk — 20% of the slow cascade).
    pub(crate) sym_case_eq: SymId,
    /// Pre-interned names for the walk-attributed fast buckets
    /// (RuboCop `Team#investigate` per-phase profile, 2026-07:
    /// `is_a?` 10.4%, `!` 6.4%, Array `include?`/`size`/`empty?`
    /// ~15%, `Kernel#Array` 4.6% of the walk's slow-cascade sends;
    /// `push`/`-@`/`<<` are the parse phase's top three).
    pub(crate) sym_not: SymId,
    pub(crate) sym_is_a: SymId,
    pub(crate) sym_kind_of: SymId,
    pub(crate) sym_push: SymId,
    pub(crate) sym_shovel: SymId,
    pub(crate) sym_neg_at: SymId,
    pub(crate) sym_kernel_array: SymId,
    pub(crate) sym_eq_op: SymId,
    pub(crate) sym_to_sym: SymId,
    /// Pre-interned names for the 2026-07 fallback-census buckets
    /// (`Array#drop`/`freeze`/`dup`, `Hash#fetch`, `String#dup`,
    /// `Object#class`, bare `block_given?` — together ~45K sends per
    /// RuboCop walk from tier-2 compiled bodies, all previously
    /// paying the full slow cascade).
    pub(crate) sym_drop: SymId,
    pub(crate) sym_fetch: SymId,
    pub(crate) sym_merge: SymId,
    pub(crate) sym_slice: SymId,
    pub(crate) sym_except: SymId,
    /// Campaign P5a: `merge!` + its CRuby alias `update` join the
    /// msx bucket (the AM census's 1,065/3K-iter residual).
    pub(crate) sym_merge_bang: SymId,
    pub(crate) sym_update: SymId,
    /// Pre-interned `hash` / `eql?` for the Hash user-key funnel gates
    /// (`key_needs_ruby_hash` scans run per merge/insert — an interner
    /// probe per call site showed up on the merge! micro).
    pub(crate) sym_key_hash: SymId,
    pub(crate) sym_key_eql: SymId,
    pub(crate) sym_freeze: SymId,
    pub(crate) sym_dup: SymId,
    pub(crate) sym_class_name: SymId,
    pub(crate) sym_block_given_q: SymId,
    /// Pre-interned names for the 2026-07 census-TAIL buckets
    /// (`Object#equal?` ~15K, `Module#method_defined?` ~27K, bare
    /// `__method__` ~16K sends per 10-iter RuboCop walk — the
    /// shapes the census wave declined at <1.2ms each; `[]=`
    /// argc-3 rides the existing `sym_index_set_op`).
    pub(crate) sym_equal_q: SymId,
    pub(crate) sym_method_defined_q: SymId,
    pub(crate) sym_method_intro: SymId,
    /// Pre-interned send-family names for the send-family fast
    /// buckets (RuboCop cop-walk census 2026-07: `respond_to?` is
    /// the single hottest slow-cascade name at ~24.7K sends per
    /// 600-line-file walk / 13.6%; `public_send` ~7.8K / 4.3%;
    /// `send` ~3.3K / 1.8% — all Sym-named, argc ≤ 3).
    pub(crate) sym_respond_to: SymId,
    /// `respond_to_missing?` — `try_respond_to_missing` probes it on
    /// EVERY respond_to? resolution miss (the common case on the
    /// RuboCop walk: `Node#loc?` probes absent selector names), so
    /// the name is pre-interned and the hook-existence probe rides
    /// the respond_to? `(class, name, method_gen)` memo.
    pub(crate) sym_respond_to_missing: SymId,
    /// The preamble's default `Object#respond_to_missing?` stub
    /// (pure `return false`), captured at `load_preamble` time —
    /// BEFORE any user code can run — so `try_respond_to_missing`
    /// can recognise "the resolution is the untouched default" by
    /// `Rc::ptr_eq` and skip the full Ruby invocation of a method
    /// that provably returns false. Holding the Rc STRONGLY pins the
    /// allocation, so a user redefinition (which replaces the table
    /// entry and bumps `method_gen`) can never alias this pointer.
    pub(crate) rtm_default_stub: Option<std::rc::Rc<crate::value::Method>>,
    pub(crate) sym_send: SymId,
    pub(crate) sym_send_u: SymId,
    pub(crate) sym_public_send: SymId,
    /// Collection-index fast-path override guard. The fast path may
    /// serve `h[k]` / `a[i]` directly ONLY while no user `[]` exists
    /// anywhere on the Hash / Array ancestor chain (a reopen, an
    /// `include`d module, or an Object-level `[]` must win — same
    /// verdict the slow path's primitive-receiver user-method gate
    /// reaches via `lookup_method_cached`). Recomputing that lookup
    /// per index call would eat the win, so the verdicts are cached
    /// here and revalidated lazily whenever `method_gen` moves —
    /// every method-table mutation path (def / define_method / alias
    /// / include / prepend / extend) already bumps `method_gen` for
    /// the inline call caches, so a stale `true` is impossible.
    /// `Vm::new` starts both gens at 0 with the flags `false` (fast
    /// path off); the preamble's method definitions bump
    /// `method_gen`, so the first user-code index call revalidates.
    pub(crate) fast_index_checked_gen: u32,
    pub(crate) fast_index_hash_safe: bool,
    pub(crate) fast_index_array_safe: bool,
    pub(crate) fast_index_hash_set_safe: bool,
    pub(crate) fast_index_array_set_safe: bool,
    /// Key-probe twin (`key?`/`has_key?`/`include?`/`member?` on
    /// Hash). Lumped across the four spellings — a user override of
    /// ANY of them on the Hash chain turns the whole probe arm off
    /// (costs only perf in that exotic program, never correctness).
    pub(crate) fast_index_hash_key_safe: bool,
    /// `try_fast_primitive` twins (same revalidation pass): no user
    /// `length`/`size`/`to_s` on String, no user `to_s`/`inspect` on
    /// Integer. Lumped per class — a user override of ANY watched
    /// name turns that class's zero-arg fast arms off (costs only
    /// perf in that exotic program, never correctness).
    pub(crate) fast_prim_str_safe: bool,
    pub(crate) fast_prim_int_safe: bool,
    /// `===` case-equality fast-path twins (same revalidation pass):
    /// no user `===` anywhere on the Symbol / String chain (sym /
    /// str flags), and no user `===` INSTANCE method on the Module /
    /// Class chain (class flag — a `class Module; def ===` reopen
    /// must fall to the slow path). A `def self.===` on a specific
    /// class is per-receiver and is checked at the call site via the
    /// IC-backed `lookup_class_singleton_cached` miss instead.
    pub(crate) fast_case_eq_sym_safe: bool,
    pub(crate) fast_case_eq_str_safe: bool,
    pub(crate) fast_case_eq_class_safe: bool,
    /// Lumped Int/Float/Bool/Nil twin: no user `===` anywhere on the
    /// Integer / Float / NilClass / TrueClass / FalseClass chains.
    /// (RuboCop's cop walk fires `Int === Int` millions of times per
    /// file via the preamble's `Enumerable#any?/none?(pattern)` /
    /// `grep` — measured the DOMINANT `===` receiver shape.)
    pub(crate) fast_case_eq_prim_safe: bool,
    /// Walk-attributed fast-bucket twins (same `method_gen`-
    /// revalidated pass as the flags above). Chain-wide
    /// `lookup_method_uncached` verdicts, mirroring the
    /// "primitive-receiver fallback to the user-Class method
    /// table" gate the slow cascade applies to Array / Symbol
    /// receivers — any user method anywhere on the chain flips
    /// the flag off and the canonical path resolves it.
    ///   - `fast_arr_read_safe`: no user `size` / `length` /
    ///     `empty?` / `include?` / `member?` on the Array chain.
    ///   - `fast_arr_push_safe` / `fast_arr_shovel_safe`: no user
    ///     `push` / `<<` on the Array chain (per-name so a
    ///     `<<`-only reopen keeps `push` fast).
    ///   - `fast_is_a_sym_safe`: no user `is_a?` / `kind_of?` on
    ///     the Symbol chain.
    ///
    /// (Int `-@`/`<<` and Bool/Nil `!` need no new flags — their
    /// guard is the existing own-table `prim_reopen_mask` bit,
    /// which is exactly the gate the cascade consults before
    /// `primitive_call` answers those names today.)
    pub(crate) fast_arr_read_safe: bool,
    pub(crate) fast_arr_push_safe: bool,
    pub(crate) fast_arr_shovel_safe: bool,
    pub(crate) fast_is_a_sym_safe: bool,
    /// Hash twin of `fast_arr_read_safe`: no user `size` / `length`
    /// / `empty?` on the Hash chain.
    pub(crate) fast_hash_read_safe: bool,
    /// NilClass twins: no user `is_a?`/`kind_of?` (resp. `==`) on
    /// the NilClass chain.
    pub(crate) fast_is_a_nil_safe: bool,
    pub(crate) fast_eq_nil_safe: bool,
    /// 2026-07 fallback-census bucket twins (ADR 0037), same
    /// method_gen-revalidated discipline:
    ///   - `fast_arr_misc_safe`: no user `drop` / `freeze` / `dup`
    ///     on the Array chain.
    ///   - `fast_hash_fetch_safe`: no user `fetch` on the Hash chain.
    ///   - `fast_str_dup_safe`: no user `dup` on the String chain.
    pub(crate) fast_arr_misc_safe: bool,
    pub(crate) fast_hash_fetch_safe: bool,
    /// Campaign-P4 bucket twin: no user `merge` / `slice` /
    /// `except` on the Hash chain (lumped — any one reopen turns
    /// all three buckets off; perf-only, never correctness).
    pub(crate) fast_hash_msx_safe: bool,
    pub(crate) fast_str_dup_safe: bool,
    /// 2026-07 P2 (AM fallback census) bucket twins, same
    /// method_gen-revalidated discipline:
    ///   - `fast_is_a_prim_safe`: no user `is_a?` / `kind_of?` on
    ///     ANY of the Integer / Float / String / TrueClass /
    ///     FalseClass chains (lumped — a reopen on one turns the
    ///     whole primitive-receiver `is_a?` bucket off; perf-only).
    ///   - `fast_kernel_array_prim_safe`: no user `to_ary` / `to_a`
    ///     on ANY of those chains nor on Symbol — the exact pair of
    ///     probes the canonical `Kernel#Array` builtin's `_` arm
    ///     makes before wrapping `[obj]`.
    pub(crate) fast_is_a_prim_safe: bool,
    pub(crate) fast_kernel_array_prim_safe: bool,
    /// TEMPORARY diagnostics (env-gated, `RUBYRS_CASCADE_STATS=1`):
    /// per-(name, receiver-shape) counters of do_call sends that
    /// reach the slow cascade (i.e. fell through every fast bucket
    /// up to the `interner.resolve` point). `None` (the default)
    /// costs one branch per slow-cascade send. Dumped to stderr by
    /// the CLI at exit; used for per-phase attribution of the
    /// RuboCop workload (parse vs cop-walk).
    pub(crate) cascade_stats: Option<Box<FxHashMap<(SymId, u8), u64>>>,
    /// TEMPORARY diagnostics (same `RUBYRS_CASCADE_STATS=1` gate):
    /// non-fixed-arity user-Ruby-method callee census, recorded at
    /// the canonical Object-recv invoke arms in the slow cascade.
    /// Key = (method name, argc passed, packed param shape,
    /// no_recv). Shape bits (LSB→): req_pre:6 | n_opt:6 | rest:1 |
    /// req_post:4 | kw_count:6 | kw_rest:1 | block_param:1 |
    /// closure:1 | non_public:1. Dumped as `nfa-stats` rows by the
    /// CLI at exit alongside `cascade-stats`.
    pub(crate) nfa_stats: Option<Box<NfaStatsMap>>,
    /// TEMPORARY census (env `RUBYRS_T2_FALLBACK_STATS=1`, ADR 0037
    /// fallback census): where the in-body calls of tier-2 compiled
    /// frames fall back to, and why. Key = (reason code, method
    /// name, receiver-shape code, min(argc,15)); the reason-code
    /// decode table lives on `Runtime::t2_fallback_stats_rows`.
    /// `None` (the default) costs one branch per t2_call fallback.
    #[cfg(feature = "jit-native")]
    pub(crate) t2_fb_stats: Option<Box<T2FbStatsMap>>,
    /// Companion census (same gate): every op a tier-2 body executes
    /// through the GENERIC helper (`t2_op`) — i.e. the op forms with
    /// no specialized serve, including the kw/splat/super call
    /// family. Key = (op variant tag, call name when one exists).
    #[cfg(feature = "jit-native")]
    pub(crate) t2_op_stats: Option<Box<T2OpStatsMap>>,
    /// One-shot marker: the NEXT `do_call`/`do_call_kw`/
    /// `do_call_block` entry is the direct fallback dispatch of a
    /// tier-2 in-body call (set at the fallback edges, taken at the
    /// dispatch entries — exact first-level attribution, nested
    /// dispatches under a served call are NOT tagged). Only ever set
    /// while `t2_fb_stats` is Some.
    #[cfg(feature = "jit-native")]
    pub(crate) t2_fb_from: bool,
    /// ADR 0031 increment 2 (plan-based): per-proto precomputed
    /// binding plans for NON-fixed-arity methods (optionals / splat
    /// / post-required / `&blk` — NOT kwargs), lazily populated by
    /// `nfa_plan_for` on the first fast-path attempt and immutable
    /// thereafter (a Proto's param shape never changes after
    /// compile). Indexed by `proto_idx`; grown on demand — protos
    /// added later (eval / require) start `Unknown`.
    pub(crate) nfa_plans: Vec<NfaPlanSlot>,
    /// Rest-predicate body-shape plans (see `RestPredPlan`), lazily
    /// verified per proto on the first NFA fast-path attempt. Same
    /// lifecycle as `nfa_plans` (a Proto's code is immutable).
    pub(crate) rest_preds: Vec<RestPredSlot>,
    /// `method_gen`-revalidated safety flag for the rest-predicate
    /// serve (recomputed in `fast_index_revalidate`, same walk):
    /// true when none of the builtin methods the verified body
    /// shapes would dispatch is user-overridden anywhere on the
    /// respective chains — `Array#include?`, `Hash#[]`,
    /// `Symbol#==`, `Symbol#nil?`, `NilClass#nil?`,
    /// `TrueClass#!`, `FalseClass#!`.
    pub(crate) rest_pred_deps_ok: bool,
    /// Env-gated (`RUBYRS_JIT_STATS`) counters for the rest-predicate
    /// serve: (served frame-free, declined-after-plan-match). Dumped
    /// with the JIT stats at exit.
    #[cfg(feature = "jit-native")]
    pub(crate) rest_pred_stats: (u64, u64),
    /// Reopen-precedence early gate (same `method_gen`-revalidated
    /// pass): bit per primitive class whose OWN method table holds
    /// at least one name a `primitive_call`-family arm claims
    /// (`primitive_arm_name_for_class`). Bits: 0=Integer, 1=Float,
    /// 2=String, 3=Symbol, 4=NilClass, 5=True/FalseClass,
    /// 6=Rational. Zero (the universal case — the preamble is
    /// audited collision-free) keeps the gate to a single u8
    /// compare per call; a set bit routes that shape through an
    /// own-table probe BEFORE the primitive arms, so `class
    /// String; def upcase; end` wins like CRuby. Own table ONLY —
    /// `include`d modules must NOT beat builtin arms (String
    /// includes Comparable; its Ruby `<` would otherwise shadow
    /// the native compare).
    /// True once ANY `undef_method` has run — gates the per-call
    /// tombstone walk in `do_call` so programs that never undef
    /// pay a single bool test (same pattern as `prim_reopen_mask`).
    pub(crate) any_undefs: bool,
    /// Union of every name EVER passed to an `undef_method`
    /// tombstone insert (both the Module arm and the
    /// `singleton_class.undef_method` host helper). Name-keyed
    /// refinement of `any_undefs` for the walk fast buckets: a
    /// tombstone can only intercept a name present in some
    /// `Class::undefed` set, so a name absent from this union can
    /// never hit one and the buckets stay live for it (previously
    /// ONE `undef_method` anywhere — ActiveSupport does several at
    /// load — turned every walk bucket off for the whole program).
    /// Insert-only; never cleared (a stale entry after
    /// redefine-over-undef only declines a bucket, the cascade
    /// re-resolves identically).
    pub(crate) undef_names: crate::intern::FxHashSet<SymId>,
    pub(crate) prim_reopen_mask: u8,
    /// True once the BASE `Array` / `Hash` / `Range` class has a
    /// user (or preamble) method in its OWN table whose name is a
    /// block-collection name (`map` / `each` / `select` / …, per
    /// `Vm::is_collection_block_name`). Block-form collection sends
    /// (`[1].map { }`) are served by the native `collection_call_block`
    /// arm, which — for a PLAIN (untagged) receiver — runs BEFORE the
    /// class-chain method lookup, so a `class Array; def map; …; end`
    /// base reopen was silently shadowed (subclass overrides go through
    /// `collection_call_block`'s `override_tag`, and the no-BLOCK path
    /// already honours base reopens via its general lookup — only the
    /// block-form base reopen was the gap). This flag is the
    /// method_gen-revalidated coarse gate (mirrors `prim_reopen_mask`):
    /// false until someone reopens a collection method, so the hot
    /// no-reopen path pays a single bool test, and only when set do the
    /// block-collection serve sites do the per-name chain lookup.
    pub(crate) coll_base_reopen: bool,
    /// Stack of Array/Hash ObjIds currently being rendered by
    /// `inspect_value`. A re-entry on an id already present is a cycle
    /// (`a = []; a << a`) and renders as `[...]` / `{...}` instead of
    /// recursing into a Rust stack overflow. LIFO matches the recursion;
    /// pushed before descending into an element, popped after.
    pub(crate) inspect_stack: Vec<ObjId>,
    /// Lazily-filled per-builtin-type `Rc<Class>` cache for `class_of`
    /// (Integer/Float/String/… — index map lives there). Reopens reuse
    /// the same Rc (DefClass `entry().or_insert_with`), so a cached
    /// entry never goes stale; only hits are cached, so calls before
    /// the preamble defines a class stay correct.
    pub(crate) builtin_class_cache: [Option<Rc<Class>>; 17],
    /// Hit/miss counters for the per-call-site IC. ZST + no-op
    /// when the `ic-stats` cargo feature is off; readable via
    /// `Runtime::ic_stats()` when on.
    pub(crate) ic_stats: IcStats,
    /// `Op::Break` sets this; iteration drivers check and consume.
    pub(crate) break_signaled: bool,
    /// `Op::ReturnMethod` sets this with the value to return. Both
    /// `dispatch` and `dispatch_until` check it at the top of every
    /// iteration: if `Some`, they unwind frames (block frames first,
    /// then one method frame) and push the value as the method's
    /// return. This is CRuby's non-local-return-from-block
    /// semantics: `return` inside a `do…end` exits the enclosing
    /// method, not just the block.
    pub(crate) method_return: Option<Value>,
    /// Stack of active `dispatch_until` boundaries. Each entry
    /// is the `until_depth` of an in-flight dispatch_until call.
    /// `Op::Raise` / `Op::EndEnsure` consult the top of this
    /// stack: when their direct call to `unwind_with_exception`
    /// redirects IP to a handler in a frame at or above that
    /// boundary, they bubble out via `RubyError::AlreadyCaught`
    /// so the native iter driver above (`Array#each`,
    /// `Hash#any?`, …) stops looping instead of pushing
    /// spurious results / re-raising on the next iteration.
    /// See [`RubyError::AlreadyCaught`] for the full protocol.
    pub(crate) dispatch_until_depths: Vec<usize>,
    /// Identity of the lexical-owner frame for an in-flight
    /// non-local return. CRuby's `return` inside a block exits
    /// the method that **lexically defined** the block, not
    /// the method that happens to be yielding. The block's
    /// `captured` Rc points at the lexical owner's locals;
    /// `Op::ReturnMethod` snapshots that Rc here so the unwind
    /// loop can identify the right method frame by
    /// `Rc::ptr_eq` (the lexical owner is the topmost
    /// non-block frame whose `locals` Rc matches). If no
    /// matching frame is found, the block escaped its lexical
    /// scope (CRuby raises LocalJumpError; Tier-1 falls back
    /// to the legacy "walk-blocks-then-pop-one-method" path
    /// to preserve existing behavior). (TRY_RUNS pass-10
    /// layer #4.)
    pub(crate) method_return_locals: Option<Rc<RefCell<Vec<Value>>>>,
    /// Free-list of recycled frame-locals cells. Every method call
    /// allocates an `Rc<RefCell<Vec<Value>>>` for its frame's locals;
    /// on return, a cell with no remaining references (`strong_count
    /// == 1` — i.e. NOT shared with a `define_method` closure capture)
    /// is cleared and parked here, so the next call reuses the
    /// allocation instead of minting a fresh one. Bounded so a deep
    /// recursion that unwinds doesn't park an unbounded pool.
    pub(crate) locals_pool: Vec<Rc<RefCell<Vec<Value>>>>,
    /// Block-frame phase profiling (see `BlockProf`).
    pub(crate) block_prof_on: bool,
    pub(crate) block_prof: BlockProf,
    /// Single-entry memo for `Op::CreateBlock`'s ancestor-chain
    /// flatten (only needed for depth ≥ 2 creations — a block created
    /// inside a chain-carrying frame), keyed by the creating frame's
    /// `(outer_rest_ptr, outer_cell_ptr, outer_cell_start)`. The
    /// flatten input is the creating frame's ROUTING structure, whose
    /// dominant shape references only stable root-scope cells (the
    /// per-invocation cell churn lives in `BlockHandle::captured`,
    /// which is NOT part of the flattened chain), so a loop that
    /// creates depth-2 closures hits this every iteration. Sound
    /// because the memoized chain holds strong Rcs to the keyed cells:
    /// the keyed addresses cannot be freed/reused while the entry
    /// lives, so a pointer match always refers to the SAME inputs —
    /// for which fresh construction would be identical. Holding dead
    /// cells until the next miss is a bounded (one entry) retention;
    /// nothing reads the memoized cells' contents through the memo.
    pub(crate) chain_memo: Option<(usize, usize, u16, crate::value::OuterChain)>,
    /// Count of LIVE `Frame::dm_share` frames (across fibers — the
    /// counter is deliberately NOT swapped by FiberStashGuard, so a
    /// dm body suspended in another fiber keeps new calls off the
    /// share path). `0` ⇒ no dm-share invocation can be clobbered ⇒
    /// the dm dispatch may share the closure cell without scanning
    /// the frame stack. Non-zero (nested / cross-fiber dm calls,
    /// rare) falls back to the per-invocation copy path — always
    /// CORRECT, just not shared-cell fast. Wholesale frame discards
    /// (`frames.clear()` / error-path truncates) recount or zero it.
    pub(crate) dm_share_depth: u32,
    /// Contiguous slot storage for `Locals::Stack` frames (the
    /// escape-analysed method-call fast path). Grows like a stack in
    /// lock-step with `frames`: a Stack frame's push appends its
    /// `n_locals` slots at the tail and records the base index; its
    /// pop truncates back to that base (LIFO holds even through
    /// exception unwind — frames pop one at a time, bases are
    /// monotonic). The WHOLE live prefix is a GC root (`gc.rs` walks
    /// it directly), which also makes values arena-resident-but-not-
    /// yet-framed (mid method-call setup) safely rooted. Fiber
    /// switches swap this Vec into `FiberSnapshot` together with
    /// `frames` — each fiber owns its own arena contents.
    pub(crate) locals_arena: Vec<Value>,
    /// Folded "any control-flow signal pending?" mask — a pure CACHE
    /// over `method_return.is_some()` / `break_signaled` /
    /// `pending_method_break.is_some()`, so the dispatch loops' hot
    /// top-of-iteration check is ONE byte test instead of three
    /// scattered Option/bool loads. Every site that mutates any of
    /// the three fields must call `sync_control_signals()` afterwards
    /// — the loop heads `debug_assert!(control_signals_synced())`, so
    /// a missed sync fails the (debug-built) test suites loudly
    /// rather than dispatching against a stale mask.
    pub(crate) control_signals: u8,
    /// In-flight `break`/`next` transfers through `ensure` chains.
    /// `Op::BreakLoop`/`Op::NextLoop` push an entry when an
    /// `is_ensure` handler sits between the source and the target;
    /// the entry pops once the transfer lands at its target loop
    /// label. `Op::EndEnsure` (emitted at the tail of every ensure
    /// handler body) resumes the TOP entry when its
    /// [`SuspendCoord`] matches the current tail position, else
    /// falls back to the exception re-raise path. A STACK (not a
    /// slot): a suspended transfer's ensure body can contain
    /// another `while … break`-through-ensure that must complete
    /// first. An exception unwinding OUT of a suspended entry's
    /// ensure body cancels that entry (CRuby: the raise wins);
    /// an exception raised AND rescued within the body leaves it
    /// alive (CRuby: the break resumes) — see
    /// `unwind_with_exception`'s escape sweeps.
    pub(crate) pending_loop_transfers: Vec<LoopTransfer>,
    /// ADR 0024 Phase A.4: in-flight block-breaks / non-local
    /// returns walking method frames' ensure chains before those
    /// frames pop. Same stack discipline + EndEnsure coordinate
    /// matching + escape cancellation as
    /// `pending_loop_transfers`; a nested entry arises when a
    /// suspended entry's ensure body calls a method that itself
    /// does a block-`return` (`def m; return 1; ensure; helper;
    /// end` where helper runs `[1].each { return 2 }`).
    pub(crate) pending_method_breaks: Vec<MethodBreak>,
    /// Monotonic counter stamped into each [`SuspendCoord`] —
    /// total order of ensure-body suspensions so `Op::EndEnsure`
    /// resumes the INNERMOST matching walk when a nested walk
    /// parked at coordinates identical to its outer walk.
    pub(crate) suspend_seq: u64,
    /// One-shot flag set by a builtin that detected its caller was
    /// unwound past its own call-site (e.g. `require_relative` saw
    /// `unwind_with_exception` route control to an outer
    /// `rescue` handler mid-load). The do_call caller checks +
    /// clears this flag before doing `stack.push(builtin_result)`;
    /// pushing in this state would corrupt the rescue handler's
    /// stack (it's already at `base_sp` after unwind truncation).
    /// Distinct from `method_return` because that path keeps frames
    /// > until_depth, while rescue unwind drops below.
    pub(crate) suppress_call_result_push: bool,
    /// Single-shot flag set by the `send` / `__send__` recogniser
    /// (`vm/dispatch.rs`) right before re-entering dispatch.
    /// Consumed (`mem::replace(..., false)`) at the **dispatch
    /// boundary** — the very top of `do_call` / `do_call_block` —
    /// into a local that the Object-arm visibility check reads.
    /// Consumption is *not* at the check site itself: that would
    /// leak the flag whenever dispatch bottoms out before the
    /// Object arm (e.g. `send(:nonexistent)` on a primitive
    /// raising NoMethodError). CRuby parity: `send` may invoke
    /// methods of any visibility, but the bypass doesn't
    /// transitively apply — anything that method itself calls
    /// runs through the normal check.
    pub(crate) bypass_visibility_once: bool,
    /// Single-shot sibling of `bypass_visibility_once` set by the
    /// `public_send` recogniser: the re-entered dispatch must
    /// enforce STRICT public visibility on the resolved method —
    /// CRuby's `public_send` raises the private/protected
    /// NoMethodError even for the literal-`self` receiver and the
    /// protected-kin caller (both exemptions that a normal
    /// explicit-receiver call honours). Same consume-at-the-
    /// dispatch-boundary discipline as `bypass_visibility_once`
    /// (leak-proof when dispatch bottoms out early), and the same
    /// non-transitivity: only the re-aimed call itself is strict.
    pub(crate) require_public_once: bool,
    /// True for the duration of a plain `Op::Call` / `Op::CallNoRecv`
    /// dispatch: the call did NOT use keyword syntax (the compiler emits
    /// `Op::Call` only when `kwargs_trailing == false`), so an explicit-
    /// brace trailing Hash (`f({k: v})`) is a POSITIONAL argument, which
    /// Ruby 3 guarantees. `invoke_method_with_block` consults this to
    /// SUPPRESS peeling the trailing Hash into keyword bindings — peeling
    /// it unconditionally was the bug that made
    /// `merge_data!({ "categories" => … })` (Liquid / Jekyll) raise
    /// `wrong number of arguments (given 0, …)`. Every OTHER call path
    /// (keyword `Op::CallKw`, splat `Op::ApplyCall`, `super`, block
    /// `Op::CallBlock`) leaves this `false`, preserving the prior
    /// "peel a trailing Hash when the callee has kwparams" behaviour.
    /// Set by the `Call` op handler, cleared once the dispatch returns
    /// (so it never leaks to the next call).
    pub(crate) trailing_hash_positional: bool,
    /// One-shot: the next `do_call` must dispatch the PRIMITIVE
    /// implementation of the method, skipping any user override on a
    /// subclassed primitive (e.g. a tagged `Hash` subclass that
    /// redefined `keys`). Set by `Op::ApplyCallPrimitive` — the body of
    /// a `<primitive-alias-forwarder>` (`alias own_keys keys` where
    /// `keys` is a primitive) — and taken at `do_call`'s top. Without
    /// it the forwarder re-dispatched by NAME and hit the user's
    /// redefinition, recursing forever (rouge/util.rb:33). CRuby's
    /// `alias` snapshots the original method; this reproduces that for
    /// the un-snapshottable primitive case.
    pub(crate) force_primitive_dispatch: bool,
    /// One-shot channel for `proc.call(args, &blk)` — the caller's
    /// block ObjId, set by the Block-recv `.call` arm in
    /// `do_call_block` and taken at `invoke_block`'s top, where it
    /// binds into the callee proto's `block_param_slot` (`|.., &b|`).
    /// Walked by the GC root gather (the window between set and
    /// frame-push crosses allocs).
    pub(crate) pending_block_arg: Option<crate::value::ObjId>,
    /// P1c.2 (ADR 0023) — fiber yield signaling slot.
    ///
    /// `Fiber.yield(v)` sets this to `Some(v)` and returns
    /// control via the next dispatch iteration. The
    /// `dispatch_until` loop checks this at the top of every
    /// iteration alongside `method_return` — when Some, the
    /// loop exits early so `resume_fiber` can observe the
    /// yield + save the suspended snapshot.
    ///
    /// `resume_fiber` clears this to None before driving and
    /// `take()`s it after the loop exits. None at all other
    /// times. Cleared by the FiberSnapshot swap on resume,
    /// so the bool semantics also survive yield/resume
    /// cycles cleanly.
    ///
    /// cfg(_fiber)-gated — Tier 1 builds never carry this
    /// field.
    #[cfg(feature = "_fiber")]
    pub(crate) fiber_yield_pending: Option<Value>,
    /// Count of LIVE native (Rust-driven) iterator block invocations
    /// — incremented/decremented around `step_block` /
    /// `step_block1` / `step_block2` (vm/iter.rs). Non-zero while a
    /// block body is executing under a Rust-level driver loop
    /// (Array#each / Integer#times / map / ...), whose loop state a
    /// `Fiber.yield` cannot stash — yielding there truncates the
    /// iteration (step_block's fiber_yield_pending guard). The
    /// `__rubyrs_fiber_can_yield` host fn compares this against the
    /// count recorded at the current fiber's resume
    /// (`FiberObject::resume_native_iter_depth`): equal ⇒ every
    /// re-entrant level above the fiber entry is a stashable shape
    /// (plain frames, Op::Yield, Proc#call), greater ⇒ a native
    /// driver is pinned and the coop scheduler must inline-drive
    /// instead of yielding (preamble/thread.rb __coop_wait_inline).
    /// Deliberately NOT cfg-gated: a plain u32 inc/dec keeps the
    /// non-fiber hot path branch-free.
    pub(crate) native_iter_depth: u32,
    /// P1c.3 (ADR 0023) — currently-running Fiber's ObjId.
    ///
    /// Set by `resume_fiber` BEFORE installing the
    /// FiberStashGuard, restored to the prior value AFTER
    /// the guard drops. `Fiber.current` reads this — at the
    /// top level (outside any Fiber) it's None and the host
    /// fn returns a sentinel "root" Value. Nested resume
    /// (Fiber A resumes Fiber B) sees the parent's id stash
    /// while B runs, and restoration on B's yield/return
    /// puts A's id back.
    ///
    /// cfg(_fiber)-gated.
    #[cfg(feature = "_fiber")]
    pub(crate) current_fiber_id: Option<ObjId>,
    /// GC visibility for the SUSPENDED-side state during a fiber
    /// resume. `FiberStashGuard::install` swaps the main program's
    /// live state (frames with all their locals, the operand stack,
    /// the pinned set, ...) out of the Vm — pre-fix it sat in a
    /// guard-owned Rust local where the GC could not see it, so ANY
    /// collection inside a fiber body swept every heap object
    /// reachable only from the suspended main program (observed as
    /// the `fiber_current_is_nil...` class_of ICE once allocation
    /// drift pushed a maybe_gc inside the body). The guard now
    /// PUSHES the outgoing snapshot here (and pops it on Drop);
    /// `gc.rs`'s root gather walks every stacked snapshot exactly
    /// like the heap-side `HeapObj::Fiber` mark arm walks suspended
    /// fibers. Nested resumes (A resumes B) stack naturally.
    #[cfg(feature = "_fiber")]
    pub(crate) fiber_stash_stack: Vec<crate::vm::fiber::FiberSnapshot>,
    /// P1d.2 (ADR 0023 v2 §"Mechanics — cext re-entrancy guard"):
    /// counter tracking depth of cext-style Vm re-entry. Each
    /// increment marks "we're inside a C extension's host fn
    /// that has re-entered the Vm via rb_funcall" (or the
    /// equivalent embed-host bridge that calls back into
    /// bytecode while a Vm borrow is mid-flight).
    ///
    /// `Fiber.yield` checks this counter and raises
    /// `FiberError("can't yield from cext")` when nonzero —
    /// without this trap, yielding mid-cext would unwind
    /// through C code that doesn't expect Ruby control flow
    /// (the suspended cext frame would be re-entered on the
    /// next resume in a state CRuby/rubyrs C extensions
    /// generally don't anticipate).
    ///
    /// Increment site: cext bridge's rb_funcall analog
    /// (vm/cext.rs) — production-side wiring lands in a
    /// follow-up cext+_fiber integration commit. P1d.2
    /// adds the field + check + protocol; tests exercise
    /// the guard by setting the counter manually.
    ///
    /// `resume_fiber` does NOT increment — it's the
    /// designed re-entry path, and fiber.resume → bytecode
    /// → Fiber.yield is the normal flow we WANT to work.
    ///
    /// Ungated (was `_fiber`-only before the production cext
    /// bridge sites started incrementing this) — the field is
    /// always present so `cext_dispatch` can drive it
    /// unconditionally. The Fiber.yield guard remains
    /// `_fiber`-gated; without `_fiber` the counter ticks but
    /// has no consumer.
    #[allow(dead_code)]
    pub(crate) cext_depth: u32,
    /// P1e.1 (ADR 0023 v2 §"Risks" #2): cap on concurrently-live
    /// Fibers. Set from `Config::max_live_fibers`.
    /// `__rubyrs_fiber_new` checks this against a heap scan of
    /// live `HeapObj::Fiber` slots; allocation past the cap
    /// raises FiberError. `None` = unlimited (default).
    /// cfg(_fiber)-gated.
    #[cfg(feature = "_fiber")]
    pub(crate) max_live_fibers: Option<usize>,
    /// P1e.2 cap mirror — see `Config::max_fiber_frame_depth`.
    /// Enforced inside `check_frames()` when
    /// `current_fiber_id.is_some()`. `None` = unlimited.
    /// cfg(_fiber)-gated.
    #[cfg(feature = "_fiber")]
    pub(crate) max_fiber_frame_depth: Option<usize>,
    /// Builtin reflection metadata for the synth Methods that
    /// `Kernel.instance_method(:foo)` returns. Looked up by the
    /// `instance_method` arm when the receiver is Kernel.
    ///
    /// Kept OUT of `Kernel.methods` deliberately: putting them on
    /// the actual chain would re-find them during regular dispatch
    /// (`obj.class` etc.) and re-invoke the synth on every call,
    /// creating either recursion or a spurious user-override
    /// signal. The registry is consulted only for the introspection
    /// surface (`instance_method` / `methods` if we ever add it),
    /// not for dispatch.
    pub(crate) kernel_builtin_metas: std::collections::HashMap<crate::intern::SymId, std::rc::Rc<crate::value::BuiltinMeta>>,
    /// Cached `Kernel` SymId, set at install time. `kernel_builtin_method`
    /// uses this for O(1) HashMap lookup into `classes` instead of a
    /// linear name-string walk.
    pub(crate) kernel_class_sym: Option<crate::intern::SymId>,
    /// BasicObject reflection metadata — same shape as Kernel but
    /// for methods CRuby defines on BasicObject (the root):
    /// `__id__`, `__send__`, `equal?`, `instance_eval`,
    /// `instance_exec`, `==`, `!=`, `!`. Kept off
    /// `BasicObject.methods` for the same reason as Kernel — see
    /// `kernel_builtin_metas`.
    pub(crate) basic_object_builtin_metas: std::collections::HashMap<crate::intern::SymId, std::rc::Rc<crate::value::BuiltinMeta>>,
    /// Cached `BasicObject` SymId — same role as `kernel_class_sym`.
    pub(crate) basic_object_class_sym: Option<crate::intern::SymId>,
    /// Reflection metadata for native `Module` instance methods
    /// (`name`, …) so `Module.instance_method(:name).bind_call(mod)`
    /// resolves — zeitwerk's `RealModName` captures `Module#name` this
    /// way to read a module's real name past any override. Same
    /// off-table design as `kernel_builtin_metas`.
    pub(crate) module_builtin_metas: std::collections::HashMap<crate::intern::SymId, std::rc::Rc<crate::value::BuiltinMeta>>,
    /// Cached `Module` SymId — same role as `kernel_class_sym`.
    pub(crate) module_class_sym: Option<crate::intern::SymId>,
    /// Cached index into `protos` of the callable→Block
    /// forwarder. Lazily built on first `&callable` coercion in
    /// `do_call_block` (BoundMethod, CurriedProc, ...). The
    /// forwarder is a tiny proto whose body does
    /// `captured[0].call(*args)`; one instance is shared across
    /// every `&` call site so the allocation cost amortises to
    /// zero.
    pub(crate) callable_forwarder_proto: Option<usize>,
    /// Cached proto for `Method#>>` / `Method#<<`. Body does
    /// `outer.call(inner.(*args))`; three-locals layout
    /// (outer / inner / rest-args). Shared across all composition
    /// sites — same amortisation rationale as the bound-method
    /// forwarder above.
    pub(crate) method_compose_forwarder_proto: Option<usize>,
    /// Filename → source-text map, populated by Runtime before
    /// each `eval`. Used by `Method#source_location` (and any
    /// future Vm-side line-resolution) to convert a Span's
    /// byte offset back to a 1-based line number. Vm-only
    /// readers; Runtime owns the canonical map and clones the
    /// `Rc<str>` source bodies in (cheap, share-pointer).
    pub(crate) sources: std::collections::HashMap<std::rc::Rc<str>, std::rc::Rc<str>>,
}


impl Vm {
    pub(crate) fn fixed_arity_for_proto(proto: &Proto, params_len: usize) -> Option<FixedArity> {
        let has_rest = proto.rest_param.is_some();
        let has_kw_rest = proto.kw_rest_param.is_some();
        let has_block_param = proto.block_param.is_some();
        let kw_count = proto.kw_param_defaults.len();
        let positional_max = params_len
            - (if has_rest { 1 } else { 0 })
            - kw_count
            - (if has_kw_rest { 1 } else { 0 })
            - (if has_block_param { 1 } else { 0 });
        let required = proto.n_required_positional as usize;
        if has_rest
            || has_kw_rest
            || has_block_param
            || kw_count != 0
            || required != positional_max
        {
            return None;
        }
        Some(FixedArity {
            required: proto.n_required_positional,
            n_locals: proto.n_locals,
            stack_eligible: !proto.creates_block,
        })
    }

    pub(crate) fn new(protos: Vec<Proto>, mut interner: Interner) -> Self {
        let sym_length = interner.intern("length");
        let sym_size = interner.intern("size");
        let sym_to_s = interner.intern("to_s");
        let sym_inspect = interner.intern("inspect");
        let sym_bang = interner.intern("$!");
        let sym_index_op = interner.intern("[]");
        let sym_index_set_op = interner.intern("[]=");
        let sym_call = interner.intern("call");
        let sym_new = interner.intern("new");
        let sym_initialize = interner.intern("initialize");
        let sym_key_q = interner.intern("key?");
        let sym_has_key_q = interner.intern("has_key?");
        let sym_include_q = interner.intern("include?");
        let sym_member_q = interner.intern("member?");
        let sym_frozen_q = interner.intern("frozen?");
        let sym_nil_q = interner.intern("nil?");
        let sym_empty_q = interner.intern("empty?");
        let sym_case_eq = interner.intern("===");
        let sym_not = interner.intern("!");
        let sym_is_a = interner.intern("is_a?");
        let sym_kind_of = interner.intern("kind_of?");
        let sym_push = interner.intern("push");
        let sym_shovel = interner.intern("<<");
        let sym_neg_at = interner.intern("-@");
        let sym_kernel_array = interner.intern("Array");
        let sym_eq_op = interner.intern("==");
        let sym_to_sym = interner.intern("to_sym");
        let sym_drop = interner.intern("drop");
        let sym_fetch = interner.intern("fetch");
        let sym_merge = interner.intern("merge");
        let sym_slice = interner.intern("slice");
        let sym_except = interner.intern("except");
        let sym_merge_bang = interner.intern("merge!");
        let sym_update = interner.intern("update");
        let sym_key_hash = interner.intern("hash");
        let sym_key_eql = interner.intern("eql?");
        let sym_freeze = interner.intern("freeze");
        let sym_dup = interner.intern("dup");
        let sym_class_name = interner.intern("class");
        let sym_block_given_q = interner.intern("block_given?");
        let sym_equal_q = interner.intern("equal?");
        let sym_method_defined_q = interner.intern("method_defined?");
        let sym_method_intro = interner.intern("__method__");
        let sym_respond_to = interner.intern("respond_to?");
        let sym_respond_to_missing = interner.intern("respond_to_missing?");
        let sym_send = interner.intern("send");
        let sym_send_u = interner.intern("__send__");
        let sym_public_send = interner.intern("public_send");
        // See the `class_singleton_deny` field doc. Union of every
        // name-keyed `do_call` arm that can fire for a Value::Class
        // receiver before the canonical user-singleton lookup, plus
        // the universal-Object names handled in the shared arms —
        // over-inclusion is harmless (slow path), under-inclusion is
        // a dispatch-precedence bug.
        // P5b name-keyed probe filter: one bit per SymId that SOME
        // name-keyed pre-cascade fast bucket in `do_call` can serve
        // (`proc.call`, `try_fast_primitive`, `try_fast_index`,
        // `try_walk_fast_buckets` incl. the hash merge/slice/except
        // bucket and the send-family re-aims). A name whose bit is
        // clear cannot be served by any of those probes, so `do_call`
        // skips them wholesale — an uncovered-shape call site stops
        // paying the whole probe wave. The receiver-shape-keyed
        // serving layers (toplevel/self/explicit-recv/class-singleton
        // ICs, the per-instance singleton gates) serve ARBITRARY
        // names and are deliberately NOT behind this mask.
        //
        // MAINTENANCE CONTRACT: adding a new name-keyed bucket to the
        // gated zone REQUIRES adding its sym here — a missed entry is
        // a silent perf loss for that bucket (never a correctness bug:
        // every gated bucket mirrors the slow cascade byte-for-byte,
        // so a skipped probe just takes the slow path).
        let probe_name_mask = {
            let served = [
                sym_call, sym_frozen_q, sym_nil_q, sym_length, sym_size,
                sym_to_s, sym_empty_q, sym_inspect, sym_index_op,
                sym_index_set_op, sym_key_q, sym_has_key_q, sym_include_q,
                sym_member_q, sym_case_eq, sym_not, sym_to_sym, sym_neg_at,
                sym_freeze, sym_dup, sym_class_name, sym_is_a, sym_kind_of,
                sym_equal_q, sym_eq_op, sym_shovel, sym_drop, sym_fetch,
                sym_push, sym_method_defined_q, sym_kernel_array,
                sym_block_given_q, sym_method_intro, sym_respond_to,
                sym_public_send, sym_send, sym_send_u, sym_merge, sym_slice,
                sym_except,
            ];
            let max = served.iter().map(|s| s.0).max().unwrap_or(0) as usize;
            let mut mask = vec![0u64; max / 64 + 1];
            for s in served {
                mask[(s.0 >> 6) as usize] |= 1u64 << (s.0 & 63);
            }
            mask
        };
        let class_singleton_deny: crate::intern::FxHashSet<SymId> = [
            "__dir__", "__send__", "send", "public_send", "method",
            "methods", "define_method", "define_singleton_method",
            "alias_method", "allocate", "new", "autoload", "autoload?",
            "const_defined?", "const_get", "const_set", "constants",
            "private_constant", "public_constant", "deprecate_constant",
            "private_class_method", "public_class_method", "include",
            "extend", "prepend", "include?", "module_function",
            "respond_to?", "respond_to_missing?", "class_eval",
            "module_eval", "instance_eval", "instance_exec",
            "instance_method", "instance_methods",
            "private_instance_methods", "public_instance_methods",
            "protected_instance_methods", "private_methods",
            "public_methods", "protected_methods", "method_defined?",
            "remove_method", "undef_method", "name", "to_s", "inspect",
            "ancestors", "superclass", "singleton_class",
            "singleton_methods", "instance_variable_get",
            "instance_variable_set", "instance_variable_defined?",
            "instance_variables", "is_a?", "kind_of?", "instance_of?",
            "class", "==", "!=", "!", "===", "=~", "equal?", "eql?",
            "nil?", "hash", "object_id", "frozen?", "freeze", "dup",
            "clone", "tap", "itself", "then", "yield_self", "display",
            "path", "private", "public", "protected", "attr_accessor",
            "attr_reader", "attr_writer", "attr",
        ]
        .into_iter()
        .map(|n| interner.intern(n))
        .collect();
        Vm {
            #[cfg(feature = "jit-native")]
            jit_native: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_zeroarg: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_objparam: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_objparam2: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_hash_scratch: None,
            #[cfg(feature = "jit-native")]
            jit_tier2_on: std::env::var_os("RUBYRS_JIT_TIER2").is_some(),
            #[cfg(feature = "jit-native")]
            jit_tier2_only: std::env::var("RUBYRS_JIT_TIER2_ONLY").ok().map(|s| {
                s.split(',')
                    .map(|n| n.trim().to_string())
                    .filter(|n| !n.is_empty())
                    .collect()
            }),
            #[cfg(feature = "jit-native")]
            t2_protos: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            t2_ptrs: Vec::new(),
            #[cfg(feature = "jit-native")]
            t2_hot: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            t2_trap: None,
            #[cfg(feature = "jit-native")]
            t2_depth: 0,
            #[cfg(feature = "jit-native")]
            t2_compile_ns: 0,
            #[cfg(feature = "jit-native")]
            jit_tier2_nocall: std::env::var_os("RUBYRS_JIT_TIER2_NOCALL").is_some(),
            #[cfg(feature = "jit-native")]
            t2_threshold_base: {
                let env_u32 = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<u32>().ok());
                env_u32("RUBYRS_JIT_TIER2_THRESHOLD")
                    .or_else(|| env_u32("RUBYRS_JIT_TIER2_BASE"))
                    .unwrap_or(T2_THRESHOLD_BASE_DEFAULT)
            },
            #[cfg(feature = "jit-native")]
            t2_threshold_per_op: {
                let env_u32 = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<u32>().ok());
                if env_u32("RUBYRS_JIT_TIER2_THRESHOLD").is_some() {
                    // Absolute override: the threshold IS the base.
                    0
                } else {
                    env_u32("RUBYRS_JIT_TIER2_PEROP").unwrap_or(T2_THRESHOLD_PER_OP_DEFAULT)
                }
            },
            #[cfg(feature = "jit-native")]
            t2_call_stats: [0; 3],
            #[cfg(feature = "jit-native")]
            jit_tier2_noblock: std::env::var_os("RUBYRS_JIT_TIER2_NOBLOCK").is_some(),
            #[cfg(feature = "jit-native")]
            t2_block_stats: [0; 3],
            #[cfg(feature = "jit-native")]
            jit_tier2_noinline: std::env::var_os("RUBYRS_JIT_TIER2_NOINLINE").is_some(),
            #[cfg(feature = "jit-native")]
            t2_site_verdict: Vec::new(),
            #[cfg(feature = "jit-native")]
            t2_poll_flags: 0,
            #[cfg(feature = "jit-native")]
            t2_lite_ptrs: Vec::new(),
            #[cfg(feature = "jit-native")]
            t2_lite_streak: Vec::new(),
            #[cfg(feature = "jit-native")]
            t2_lite_stats: [0; 3],
            #[cfg(feature = "jit-native")]
            t2_lite_blk_ptrs: Vec::new(),
            #[cfg(feature = "jit-native")]
            jit_tier2_noliteblk: std::env::var_os("RUBYRS_JIT_TIER2_NOLITEBLK").is_some(),
            #[cfg(feature = "jit-native")]
            t2_lite_blk_stats: [0; 2],
            #[cfg(feature = "jit-native")]
            restblk_census: [0; 6],
            #[cfg(feature = "jit-native")]
            restblk_census_by: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            symproc_serves: 0,
            #[cfg(feature = "jit-native")]
            jit_tier2_nolite: std::env::var_os("RUBYRS_JIT_TIER2_NOLITE").is_some(),
            #[cfg(feature = "jit-native")]
            t2_lite_pending: Vec::new(),
            #[cfg(feature = "jit-native")]
            t2_lite_dc: None,
            #[cfg(feature = "jit-native")]
            t2_lite_call_stats: [0; 5],
            #[cfg(feature = "jit-native")]
            jit_native_fparam: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_poly: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_value: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_block: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_sum_loop: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_objmethod_sum_loop: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_map_loop: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_block_pred: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_count_loop: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_filter_loop: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_find_loop: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_block2: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_inject_loop: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_block2_finject: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_finject_loop: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_block_acc: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_each_acc_loop: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_block_eachobj: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_eachobj_loop: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_block_eachobj_f: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_eachobj_loop_f: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_block_eachobjhash: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_eachobjhash_loop: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_groupby_loop: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_block_eachidx_k: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_eachidx_loop_k: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_block_float: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_floatsum_loop: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_block_intelem_fa: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_intelem_floatsum_loop: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_block_floatint: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_floatint_sum_loop: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_floatint_map_loop: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_floatint_groupby_loop: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_floatkey_groupby_loop: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_intelem_floatmap_loop: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_floatmap_loop: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_block_acc_float: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_floateach_acc_loop: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_block_acc_intelem_fa: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_intelem_floateach_acc_loop: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_minmax_loop: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_floatminmax_loop: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_intelem_floatminmax_loop: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_block_pred_float: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_floatcount_loop: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_floatfilter_loop: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_floatfind_loop: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_on: std::env::var_os("RUBYRS_JIT_NATIVE").is_some(),
            #[cfg(feature = "jit-native")]
            jit_stats_on: std::env::var_os("RUBYRS_JIT_STATS").is_some(),
            #[cfg(feature = "jit-native")]
            jit_stats: JitStats::default(),
            #[cfg(feature = "jit-native")]
            jit_deopt_count: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_flags: Vec::new(),
            protos,
            interner,
            classes: FxHashMap::default(),
            constants: FxHashMap::default(),
            const_source_locations: FxHashMap::default(),
            #[cfg(not(target_os = "wasi"))]
            loaded_features: std::collections::HashSet::new(),
            #[cfg(not(target_os = "wasi"))]
            completed_features: std::collections::HashSet::new(),
            #[cfg(not(target_os = "wasi"))]
            loaded_stdlib_stubs: std::collections::HashSet::new(),
            #[cfg(not(target_os = "wasi"))]
            autoloads_toplevel: HashMap::new(),
            #[cfg(not(target_os = "wasi"))]
            autoloads_scoped: HashMap::new(),
            consumed_autoloads: std::collections::HashSet::new(),
            private_consts: std::collections::HashSet::new(),
            autoload_paths: std::collections::HashMap::new(),
            cache_counter: crate::compiler::CidGen::default(),
            globals: FxHashMap::default(),
            toplevel_methods: FxHashMap::default(),
            main_obj: None,
            toplevel_cvars: HashMap::new(),
            load_path: None,
            loaded_features_list: None,
            host_fns: HashMap::new(),
            #[cfg(feature = "cext")]
            cext_class_methods: HashMap::new(),
            #[cfg(all(feature = "cext", not(target_os = "wasi")))]
            cext_instance_methods: HashMap::new(),
            class_stack: vec![],
            class_visibility_stack: vec![],
            module_function_active_stack: vec![],
            #[cfg(feature = "regex")]
            regex_cache: HashMap::new(),
            #[cfg(feature = "regex")]
            last_match: None,
            #[cfg(feature = "bignum")]
            bigint_lit_cache: HashMap::new(),
            env_hash: None,
            env_override: None,
            pid: None,
            time_now: None,
            sleep_for: None,
            process_exit: None,
            // Default to a fresh dedicated flag. Runtime
            // construction may replace this with the shared
            // process-wide Arc if any Runtime opted into signal
            // handling.
            interrupt_pending: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            suppress_interrupt: 0,
            signal_traps: std::collections::HashMap::new(),
            yield_recursion_depth: 0,
            // None until Config::apply lays in a value. The
            // safe-point check treats None as "unlimited".
            max_yield_recursion: None,
            at_exit_handlers: Vec::new(),
            last_uncaught_exception: None,
            marshal_registry: Vec::new(),
            anon_class_counter: 0,
            any_hash_singletons: false,
            str_singletons: crate::intern::FxHashMap::default(),
            any_str_singletons: false,
            str_ivars: crate::intern::FxHashMap::default(),
            any_str_ivars: false,
            heap_singletons: crate::intern::FxHashMap::default(),
            any_heap_singletons: false,
            binding_locals: crate::intern::FxHashMap::default(),
            default_external: crate::value::EncodingTag::Utf8,
            default_internal: None,
            module_refinements: crate::intern::FxHashMap::default(),
            refinement_targets: crate::intern::FxHashMap::default(),
            active_refinements: crate::intern::FxHashMap::default(),
            refined_method_names: crate::intern::FxHashSet::default(),
            stack: Vec::with_capacity(1024),
            frames: vec![],
            frame_aux_pool: Vec::new(),
            heap: Heap::new(),
            pinned: Vec::new(),
            // ADR 0017 Rule 2 closure: default sink is silent
            // (`std::io::sink()`); hosts that want script output
            // routed somewhere call `Runtime::set_stdout` explicitly.
            // The CLI binary `rubyrs` wires it to process stdout in
            // `main.rs` so `rubyrs script.rb` behaves like CRuby.
            stdout: Box::new(std::io::sink()),
            stderr: Box::new(std::io::sink()),
            // Default to false; Config-driven `stress_gc` flows in
            // via `Runtime::apply_config`. The previous `env::var`
            // read here meant `Vm::new` indirectly hit a wasi
            // import on wasm32-wasip1, which would violate wizer's
            // "no imports during init" rule (see PR #116 review)
            // and bake the wizer-time env into the snapshot rather
            // than respecting the user's runtime `STRESS_GC` setting.
            // Now the env read happens exactly once at the CLI
            // boundary (`main.rs::env_lookup("STRESS_GC")`), feeds
            // into `Config.stress_gc`, and reaches the Vm via
            // `apply_config`.
            stress_gc: false,
            // Secure-by-default — matches Config::default's
            // `allow_filesystem_io: false`. CLI / FS-needing
            // embedders flip this via `apply_config`.
            allow_filesystem_io: false,
            allow_process_spawn: false,
            // No path narrowing by default — `allow_filesystem_io: false`
            // already covers the secure-by-default sandbox.
            allowed_paths: None,
            #[cfg(feature = "_sqlite")]
            sqlite_allow_paths: None,
            #[cfg(feature = "_socket")]
            allow_network_io: false,
            #[cfg(feature = "_socket")]
            socket_allow_hosts: None,
            #[cfg(feature = "_socket")]
            socket_max_read_bytes: None,
            #[cfg(feature = "_sqlite")]
            sqlite_max_result_bytes: None,
            fuel: None,
            max_frames: None,
            max_dispatch_depth: None,
            deadline_at: None,
            op_counter: 0,
            max_symbols: None,
            max_value_bytes: None,
            call_caches: Vec::new(),
            ivar_caches: Vec::new(),
            cvar_caches: Vec::new(),
            cvar_gen: 0,
            super_caches: Vec::new(),
            method_gen: 0,
            const_cache_flat: FxHashMap::default(),
            const_cache_chain: FxHashMap::default(),
            const_gen: 0,
            sym_length,
            class_singleton_deny,
            probe_name_mask,
            sym_size,
            sym_bang,
            sym_index_op,
            sym_index_set_op,
            sym_call,
            cwd_cache: None,
            sym_new,
            sym_initialize,
            sym_key_q,
            sym_has_key_q,
            sym_include_q,
            sym_member_q,
            sym_frozen_q,
            sym_nil_q,
            sym_empty_q,
            sym_case_eq,
            sym_not,
            sym_is_a,
            sym_kind_of,
            sym_push,
            sym_shovel,
            sym_neg_at,
            sym_kernel_array,
            sym_eq_op,
            sym_to_sym,
            sym_drop,
            sym_fetch,
            sym_merge,
            sym_slice,
            sym_except,
            sym_merge_bang,
            sym_update,
            sym_key_hash,
            sym_key_eql,
            sym_freeze,
            sym_dup,
            sym_class_name,
            sym_block_given_q,
            sym_equal_q,
            sym_method_defined_q,
            sym_method_intro,
            sym_respond_to,
            sym_respond_to_missing,
            rtm_default_stub: None,
            sym_send,
            sym_send_u,
            sym_public_send,
            fast_index_checked_gen: 0,
            fast_index_hash_safe: false,
            fast_index_array_safe: false,
            fast_index_hash_set_safe: false,
            fast_index_array_set_safe: false,
            fast_index_hash_key_safe: false,
            fast_prim_str_safe: false,
            fast_prim_int_safe: false,
            fast_case_eq_sym_safe: false,
            fast_case_eq_str_safe: false,
            fast_case_eq_class_safe: false,
            fast_case_eq_prim_safe: false,
            fast_arr_read_safe: false,
            fast_arr_push_safe: false,
            fast_arr_shovel_safe: false,
            fast_is_a_sym_safe: false,
            fast_hash_read_safe: false,
            fast_is_a_nil_safe: false,
            fast_eq_nil_safe: false,
            fast_arr_misc_safe: false,
            fast_hash_fetch_safe: false,
            fast_hash_msx_safe: false,
            fast_str_dup_safe: false,
            fast_is_a_prim_safe: false,
            fast_kernel_array_prim_safe: false,
            cascade_stats: if std::env::var_os("RUBYRS_CASCADE_STATS").is_some() {
                Some(Box::default())
            } else {
                None
            },
            nfa_stats: if std::env::var_os("RUBYRS_CASCADE_STATS").is_some() {
                Some(Box::default())
            } else {
                None
            },
            #[cfg(feature = "jit-native")]
            t2_fb_stats: if std::env::var_os("RUBYRS_T2_FALLBACK_STATS").is_some() {
                Some(Box::default())
            } else {
                None
            },
            #[cfg(feature = "jit-native")]
            t2_op_stats: if std::env::var_os("RUBYRS_T2_FALLBACK_STATS").is_some() {
                Some(Box::default())
            } else {
                None
            },
            #[cfg(feature = "jit-native")]
            t2_fb_from: false,
            nfa_plans: Vec::new(),
            rest_preds: Vec::new(),
            rest_pred_deps_ok: false,
            #[cfg(feature = "jit-native")]
            rest_pred_stats: (0, 0),
            any_undefs: false,
            undef_names: crate::intern::FxHashSet::default(),
            prim_reopen_mask: 0,
            coll_base_reopen: false,
            inspect_stack: Vec::new(),
            builtin_class_cache: Default::default(),
            sym_to_s,
            sym_inspect,
            ic_stats: IcStats::new(),
            break_signaled: false,
            callable_forwarder_proto: None,
            method_compose_forwarder_proto: None,
            sources: HashMap::new(),
            method_return: None,
            dispatch_until_depths: Vec::new(),
            method_return_locals: None,
            locals_pool: Vec::new(),
            block_prof_on: std::env::var_os("RUBYRS_BLOCK_PROF").is_some(),
            block_prof: BlockProf::default(),
            chain_memo: None,
            dm_share_depth: 0,
            locals_arena: Vec::new(),
            control_signals: 0,
            pending_loop_transfers: Vec::new(),
            pending_method_breaks: Vec::new(),
            suspend_seq: 0,
            suppress_call_result_push: false,
            bypass_visibility_once: false,
            require_public_once: false,
            trailing_hash_positional: false,
            force_primitive_dispatch: false,
            pending_block_arg: None,
            #[cfg(feature = "_fiber")]
            fiber_yield_pending: None,
            native_iter_depth: 0,
            #[cfg(feature = "_fiber")]
            current_fiber_id: None,
            #[cfg(feature = "_fiber")]
            fiber_stash_stack: Vec::new(),
            cext_depth: 0,
            #[cfg(feature = "_fiber")]
            max_live_fibers: None,
            #[cfg(feature = "_fiber")]
            max_fiber_frame_depth: None,
            kernel_builtin_metas: std::collections::HashMap::new(),
            kernel_class_sym: None,
            basic_object_builtin_metas: std::collections::HashMap::new(),
            basic_object_class_sym: None,
            module_builtin_metas: std::collections::HashMap::new(),
            module_class_sym: None,
        }
    }





}


impl Vm {

    /// Get-or-create the TOP frame's aux box, reusing a pooled box
    /// when one is available (see `frame_aux_pool`). The hot aux
    /// creators (`Op::EnterBegin` / `Op::PushRescue` / `Op::PushEnsure`
    /// / `Op::EnterLoop`) route through here so a begin/rescue-bearing
    /// method stays malloc-free per call once the pool is warm.
    #[inline]
    pub(crate) fn top_aux_mut(&mut self) -> &mut FrameAux {
        let f = self.frames.last_mut().expect("ICE: aux access with no frame");
        if f.aux.is_none() {
            f.aux = Some(self.frame_aux_pool.pop().unwrap_or_default());
        }
        f.aux.as_mut().unwrap()
    }

    /// Return a popped frame's aux box to the pool: clear every field
    /// (keeping the Vec capacities — that's the point) so the pool
    /// holds no `Value`s / stale state, and cap the pool size so a
    /// deep-recursion spike doesn't pin memory forever. Frame-pop
    /// sites call this best-effort; a frame dropped elsewhere (e.g.
    /// `frames.truncate` on an error path) simply frees its box.
    #[inline]
    pub(crate) fn recycle_frame_aux(&mut self, aux: Option<Box<FrameAux>>) {
        const FRAME_AUX_POOL_MAX: usize = 32;
        if let Some(mut a) = aux
            && self.frame_aux_pool.len() < FRAME_AUX_POOL_MAX
        {
            a.invoked_name = None;
            a.instance_eval_definee = None;
            a.rescues.clear();
            a.loop_rescue_depths.clear();
            a.loop_stack_depths.clear();
            a.begin_rescue_depths.clear();
            self.frame_aux_pool.push(a);
        }
    }

    /// Record a native-JIT method EXECUTION attempt (family per
    /// `JIT_FAM_NAMES`); `deopt` = the native code bailed and the interpreter
    /// re-ran the body. No-op unless `RUBYRS_JIT_STATS` is set.
    #[cfg(feature = "jit-native")]
    #[inline]
    pub(crate) fn jstat_exec(&mut self, proto_idx: usize, fam: u8, deopt: bool) {
        if !self.jit_stats_on {
            return;
        }
        let e = self.jit_stats.exec.entry((proto_idx, fam)).or_insert((0, 0));
        e.0 += 1;
        if deopt {
            e.1 += 1;
        }
    }

    /// Shrink `protos` to `new_len` and every proto_idx-INDEXED side
    /// table in lockstep. The side tables' "index = proto_idx,
    /// immutable once compiled" lifecycle only holds while the proto
    /// Vec never shrinks; `Runtime::reset()` is exactly the shrink
    /// (it rewinds to the post-preamble baseline and the next eval's
    /// compiler hands out the same indices again). A recycled index
    /// paired with a stale side-table entry served the PREVIOUS
    /// eval's lazily-built arg-binding plan / JIT verdict for a
    /// brand-new method — the 2026-07-05 fuzz-soak crash family
    /// (locals_arena OOB stores/loads, args-shuffle subtract
    /// overflow, "CreateBlock in a Locals::Stack frame" ICEs).
    ///
    /// Owning the sweep HERE (not scattered in reset()) is the
    /// guard-rail for the next proto-indexed table: add its
    /// truncate/retain to this function when you add the field.
    ///
    /// NOT swept (documented carve-out): the ~40 per-shape
    /// `jit_native_*` verdict maps, `jit_value`, and `restblk_census_by`
    /// are proto_idx-keyed too, but sweeping them is only observable
    /// to an embedder that runs a reset() loop WITH the opt-in
    /// native JIT enabled — no such embedder exists today (the fuzz
    /// harness and per-request hosts run interpreter-only). When
    /// that combination becomes real, extend this sweep before
    /// enabling it; a stale native body on a recycled index executes
    /// the previous eval's compiled code.
    pub(crate) fn truncate_protos(&mut self, new_len: usize) {
        self.protos.truncate(new_len);
        self.nfa_plans.truncate(new_len);
        self.rest_preds.truncate(new_len);
        #[cfg(feature = "jit-native")]
        {
            self.jit_flags.truncate(new_len);
            self.t2_ptrs.truncate(new_len);
            self.t2_lite_ptrs.truncate(new_len);
            self.t2_lite_blk_ptrs.truncate(new_len);
            self.t2_lite_streak.truncate(new_len);
            self.t2_protos.retain(|&pidx, _| pidx < new_len);
            self.t2_hot.retain(|&pidx, _| pidx < new_len);
            self.jit_deopt_count.retain(|&(pidx, _), _| pidx < new_len);
        }
    }

    /// Read the proto's JIT dispatch flags (`JFLAG_*`); 0 = nothing settled.
    #[cfg(feature = "jit-native")]
    #[inline]
    pub(crate) fn jit_flags_get(&self, proto_idx: usize) -> u8 {
        self.jit_flags.get(proto_idx).copied().unwrap_or(0)
    }

    /// OR a `JFLAG_*` bit into the proto's flag byte (grows the table).
    #[cfg(feature = "jit-native")]
    pub(crate) fn jit_flags_set(&mut self, proto_idx: usize, bit: u8) {
        if self.jit_flags.len() <= proto_idx {
            self.jit_flags.resize(proto_idx + 1, 0);
        }
        self.jit_flags[proto_idx] |= bit;
    }

    /// TIER-2 serving hook (ADR 0037), called RIGHT AFTER a method frame is
    /// pushed at the dispatch fast paths: run the just-pushed top frame's
    /// body natively when a tier-2 compile exists (compiling it on the
    /// `T2_COMPILE_THRESHOLD`-th entry). On return the VM state is exactly
    /// what the interpreter would produce: either the frame completed (popped,
    /// result on the operand stack), or it bailed with `frame.ip` at the
    /// resume point (the master loop continues interpreting — a mode switch,
    /// never a re-execution), or a Trap propagates. Precedence: the frameless
    /// specialized tiers (int/value/objparam/zeroarg/getter) serve BEFORE any
    /// frame is pushed, so tier-2 only ever sees what they declined.
    #[cfg(feature = "jit-native")]
    #[inline]
    pub(crate) fn t2_enter(&mut self) -> Result<(), crate::error::Trap> {
        if !self.jit_tier2_on {
            return Ok(());
        }
        self.t2_enter_slow().map(|_served| ())
    }

    /// TIER-2 BLOCK serving hook (ADR 0037 wave 5), called RIGHT AFTER the
    /// interpreter's block binders (`invoke_block`/`invoke_block1`/
    /// `invoke_block2`) pushed a block frame at the hot invocation sites
    /// (the `Op::Yield` arm, the `step_block` family, the `proc.call`
    /// arms): run the just-pushed block frame's body natively when a
    /// tier-2 compile exists. Identical serving discipline to `t2_enter`
    /// — param binding (autosplat, rest, kw, block-param, lambda arity)
    /// already happened in the interpreter's own binder, so the compiled
    /// body starts at op 0 with the frame exactly as interpretation would
    /// see it; a bail is a mode switch (the caller's `dispatch_until`
    /// continues the frame at `ip`), never a re-execution. `from_yield`
    /// only labels the stats counter (native-yield count).
    #[cfg(feature = "jit-native")]
    #[inline]
    pub(crate) fn t2_enter_block(&mut self, from_yield: bool) -> Result<(), crate::error::Trap> {
        if !self.jit_tier2_on || self.jit_tier2_noblock {
            return Ok(());
        }
        if self.jit_stats_on {
            self.t2_block_stats[0] += 1;
        }
        let served = self.t2_enter_slow()?;
        if served && self.jit_stats_on {
            self.t2_block_stats[1] += 1;
            if from_yield {
                self.t2_block_stats[2] += 1;
            }
        }
        Ok(())
    }

    /// Returns whether the top frame was actually run natively (`Ok(true)`)
    /// or left for the interpreter (`Ok(false)`); `t2_enter` ignores the
    /// flag, `t2_enter_block` feeds its stats counters from it.
    #[cfg(feature = "jit-native")]
    fn t2_enter_slow(&mut self) -> Result<bool, crate::error::Trap> {
        let pidx = match self.frames.last() {
            Some(f) => f.proto_idx,
            None => return Ok(false),
        };
        let flags = self.jit_flags_get(pidx);
        if flags & JFLAG_NO_TIER2 != 0 {
            return Ok(false);
        }
        if self.t2_depth >= T2_MAX_NATIVE_DEPTH {
            return Ok(false); // deep recursion: interpret (flat loop, no Rust stack)
        }
        if flags & JFLAG_TIER2_HAS == 0 {
            let c = self.t2_hot.entry(pidx).or_insert(0);
            *c += 1;
            // Adaptive threshold (wave 2, compile-cost control): scale the
            // required entries with body size — compile cost is ~linear in
            // ops while per-entry savings are ~flat, so bigger bodies must
            // prove more heat before paying the Cranelift bill.
            let threshold = self
                .t2_threshold_base
                .saturating_add(
                    self.t2_threshold_per_op
                        .saturating_mul(self.protos[pidx].code.len() as u32),
                );
            if *c < threshold {
                return Ok(false);
            }
            if let Some(only) = &self.jit_tier2_only
                && !only.contains(&self.protos[pidx].name)
            {
                self.jit_flags_set(pidx, JFLAG_NO_TIER2);
                return Ok(false);
            }
            if self.jit_stats_on {
                self.jit_stats.compile[7][0] += 1;
            }
            let compile_t0 = self
                .jit_stats_on
                .then(std::time::Instant::now);
            let t2ctx = crate::jit_tier2::T2Ctx {
                nocall: self.jit_tier2_nocall,
                noinline: self.jit_tier2_noinline,
                nolite: self.jit_tier2_nolite,
                interrupt_addr: std::sync::Arc::as_ptr(&self.interrupt_pending) as usize,
                sym_nil_q: self.sym_nil_q.0,
            };
            match crate::jit_tier2::compile_tier2(&self.protos[pidx], pidx, &t2ctx)
            {
                Some(p) => {
                    let entry = p.ptr;
                    let lite = p.lite_ptr;
                    let lite_blk = p.lite_blk_ptr;
                    self.t2_protos.insert(pidx, p);
                    if self.t2_ptrs.len() <= pidx {
                        self.t2_ptrs.resize(pidx + 1, None);
                    }
                    self.t2_ptrs[pidx] = Some(entry);
                    // Wave-4 frame-lite entry: served at the fixed-arity
                    // dispatch fast paths BEFORE any frame is pushed.
                    if let Some(lp) = lite {
                        if self.t2_lite_ptrs.len() <= pidx {
                            self.t2_lite_ptrs.resize(pidx + 1, None);
                        }
                        self.t2_lite_ptrs[pidx] = Some(lp);
                        self.jit_flags_set(pidx, JFLAG_TIER2_LITE);
                    }
                    // LITE-BLOCK entry: served by the block-invocation
                    // sites (`invoke_block1`'s frameless arm) BEFORE the
                    // block frame is built.
                    if let Some(lbp) = lite_blk {
                        if self.t2_lite_blk_ptrs.len() <= pidx {
                            self.t2_lite_blk_ptrs.resize(pidx + 1, None);
                        }
                        self.t2_lite_blk_ptrs[pidx] = Some(lbp);
                        self.jit_flags_set(pidx, JFLAG_TIER2_LITEBLK);
                    }
                    self.jit_flags_set(pidx, JFLAG_TIER2_HAS);
                    if self.jit_stats_on {
                        self.jit_stats.compile[7][1] += 1;
                    }
                }
                None => {
                    self.jit_flags_set(pidx, JFLAG_NO_TIER2);
                    return Ok(false);
                }
            }
            if let Some(t0) = compile_t0 {
                self.t2_compile_ns += t0.elapsed().as_nanos() as u64;
            }
        }
        // The fn pointer is copied out BEFORE running (the machine code lives
        // in the module's mmap and never moves; the dense table entry is a
        // copy, so a nested compile growing `t2_ptrs` can't invalidate it —
        // same discipline as `NpEntry`).
        let f = match self.t2_ptrs.get(pidx).copied().flatten() {
            Some(f) => f,
            None => return Ok(false),
        };
        // native→native accounting: entering a tier-2 body while already
        // inside tier-2 native code (`t2_depth > 0`) is the wave-2 direct
        // native call chain (t2_call → frame push → this entry).
        if self.jit_stats_on && self.t2_depth > 0 {
            self.t2_call_stats[2] += 1;
        }
        // Wave-3 backward-branch poll gate: fuel/deadline activation only
        // changes between evals, so a per-serve recompute can never be
        // stale while native code runs.
        self.t2_poll_flags = (self.fuel.is_some() || self.deadline_at.is_some()) as u8;
        // TIER-2 kwargs-super correctness (ADR 0037): the just-pushed frame's
        // body runs INLINE here, still inside the caller's `do_call` — i.e.
        // BEFORE the `Op::Call`-family arm's post-dispatch
        // `trailing_hash_positional = false` reset (step.rs) that the
        // read-only fast-path binders (`try_invoke_nfa_method_from_stack`)
        // deliberately defer to. The interpreter's dispatch loop only reaches
        // a method body AFTER that reset, so a bare `super` forwarding kwargs
        // (compiler rebuilds a trailing kwargs Hash + `ApplySuper`, which
        // peels iff `!trailing_hash_positional`) sees the flag FALSE there.
        // Restore that invariant for the inline native run: clear the flag so
        // the compiled body observes the same post-binder state. Without this
        // a hot `def m(a:); super; end` tier-2-compiles and forwards the Hash
        // as a POSITIONAL arg → "wrong number of arguments (given 1,
        // expected 0)" (`super_forward_kwargs.rb`).
        self.trailing_hash_positional = false;
        self.t2_depth += 1;
        let status = f(self as *mut Vm);
        self.t2_depth -= 1;
        if self.jit_stats_on {
            self.jstat_exec(pidx, 7, status == crate::jit_tier2::T2_BAIL);
        }
        if status == crate::jit_tier2::T2_TRAP {
            return Err(self
                .t2_trap
                .take()
                .expect("ICE: tier-2 trap status without a stored trap"));
        }
        Ok(true)
    }

    /// Wave-4 FRAME-LITE serve (ADR 0037): run `pidx`'s frameless variant
    /// against the current operand stack — recv (when `has_recv`) and the
    /// `argc` args stay ON the stack (rooted) for the whole run; NO frame is
    /// pushed, no args are bound. Call sites are the fixed-arity dispatch
    /// fast paths, AFTER their `check_frames` and arity checks and INSTEAD
    /// of the bind+push+`t2_enter` sequence. Contract on return:
    ///
    /// - `T2_DONE`: recv+args were replaced by the return value — the call
    ///   is complete (the site returns `Ok(true)`).
    /// - `T2_BAIL`: the native code MATERIALIZED the real frame (the
    ///   deferred push: current locals bound, recv+args consumed, `ip` at
    ///   the resume op) — the site returns `Ok(true)` and its caller
    ///   continues the frame exactly like any interpreter push (the master
    ///   loop, or a tier-2 caller's `dispatch_until`). A mode switch, never
    ///   a re-execution.
    ///
    /// `self_words` is a borrowing view of the receiver (the stack slot /
    /// the caller frame's `self_val` / the site's owned local keeps it
    /// rooted — no GC can run inside a lite body anyway: no admitted helper
    /// allocates).
    #[cfg(feature = "jit-native")]
    #[inline]
    pub(crate) fn t2_lite_run(
        &mut self,
        f: crate::jit_tier2::T2LiteFn,
        pidx: usize,
        self_words: [i64; 2],
        n_pop: usize,
        dc: Option<std::rc::Rc<crate::value::Class>>,
    ) -> Result<(), crate::error::Trap> {
        // Backward-branch poll gate mirror — same per-serve recompute as
        // `t2_enter_slow` (a fired gate materializes + bails; delivery
        // stays owned by the dispatch loop heads).
        self.t2_poll_flags = (self.fuel.is_some() || self.deadline_at.is_some()) as u8;
        // Deferred-push `defining_class` hand-off (see `t2_lite_dc`): a
        // materialize consumes it; a completed serve clears it below.
        // Serve sites can only be reached with NO pending lite chain (a
        // frameless activation never re-enters a dispatch site).
        debug_assert!(self.t2_lite_pending.is_empty(), "ICE: lite serve inside a lite chain");
        self.t2_lite_dc = dc;
        // Native-nesting accounting: lite→lite chains inside this run
        // deepen the Rust stack; share the tier-2 cap.
        self.t2_depth += 1;
        let status = f(self as *mut Vm, self_words[0], self_words[1], n_pop as i64);
        self.t2_depth -= 1;
        if status == crate::jit_tier2::T2_DONE {
            self.t2_lite_dc = None;
            if self.jit_stats_on {
                self.t2_lite_stats[0] += 1;
                self.jstat_exec(pidx, 8, false);
            }
            if let Some(s) = self.t2_lite_streak.get_mut(pidx) {
                *s = 0;
            }
            return Ok(());
        }
        debug_assert_eq!(status, crate::jit_tier2::T2_BAIL, "ICE: lite status");
        debug_assert!(self.t2_lite_dc.is_none(), "ICE: lite bail left dc unconsumed");
        // Materialize-bail: the frame exists now. Breaker attribution
        // happened at the materialize itself (`lite_materialize_core`),
        // against the proto whose shape actually failed — which, with
        // lite→lite chains, may be a CALLEE rather than this entry proto.
        Ok(())
    }

    /// LITE-BLOCK serve (ADR 0037 block-frame residue): run `pidx`'s
    /// frameless BLOCK variant. The serve site (invoke_block1's frameless
    /// arm) has already pushed the block's bound arg(s) onto the operand
    /// stack (rooted; `n_pop = n_params` is baked into the entry) and
    /// checked the handle against the baked call shape. `self_words`
    /// borrows the handle's `self_val` (the handle outlives the frameless
    /// window — no GC can run inside it). Same DONE/BAIL contract as
    /// `t2_lite_run`; `defining_class` is `None` by block-frame
    /// construction, so the hand-off slot stays empty.
    #[cfg(feature = "jit-native")]
    #[inline]
    pub(crate) fn t2_lite_run_blk(
        &mut self,
        f: crate::jit_tier2::T2LiteBlkFn,
        pidx: usize,
        self_words: [i64; 2],
        block_id: crate::value::ObjId,
    ) {
        self.t2_poll_flags = (self.fuel.is_some() || self.deadline_at.is_some()) as u8;
        debug_assert!(self.t2_lite_pending.is_empty(), "ICE: lite-blk serve inside a lite chain");
        if self.jit_stats_on {
            self.t2_lite_blk_stats[0] += 1;
        }
        self.t2_depth += 1;
        let status = f(self as *mut Vm, self_words[0], self_words[1], block_id.0 as i64);
        self.t2_depth -= 1;
        if status == crate::jit_tier2::T2_DONE {
            if self.jit_stats_on {
                self.t2_lite_blk_stats[1] += 1;
                self.jstat_exec(pidx, 8, false);
            }
            if let Some(s) = self.t2_lite_streak.get_mut(pidx) {
                *s = 0;
            }
            return;
        }
        debug_assert_eq!(status, crate::jit_tier2::T2_BAIL, "ICE: lite-blk status");
        debug_assert!(self.t2_lite_dc.is_none(), "ICE: lite-blk bail left dc set");
    }

    /// Breaker bookkeeping for a lite materialize-bail — called by
    /// `jit_tier2::lite_materialize_core` for the proto that materialized
    /// ITSELF (cascade-drained callers are not charged): bump the proto's
    /// consecutive-bail streak, kill the lite entry at
    /// `T2_LITE_KILL_STREAK` (chronic shape mismatch = wasted entry +
    /// materialize per call).
    #[cfg(feature = "jit-native")]
    pub(crate) fn t2_lite_note_bail(&mut self, pidx: usize) {
        if self.t2_lite_streak.len() <= pidx {
            self.t2_lite_streak.resize(pidx + 1, 0);
        }
        if self.jit_stats_on {
            self.jstat_exec(pidx, 8, true);
        }
        let s = &mut self.t2_lite_streak[pidx];
        *s = s.saturating_add(1);
        if *s >= T2_LITE_KILL_STREAK {
            if let Some(slot @ Some(_)) = self.t2_lite_ptrs.get_mut(pidx) {
                *slot = None; // module stays alive in t2_protos
                self.t2_lite_stats[2] += 1; // count actual kills once
            }
            if let Some(slot @ Some(_)) = self.t2_lite_blk_ptrs.get_mut(pidx) {
                *slot = None;
                self.t2_lite_stats[2] += 1;
            }
            if let Some(f) = self.jit_flags.get_mut(pidx) {
                *f &= !(JFLAG_TIER2_LITE | JFLAG_TIER2_LITEBLK);
            }
        }
    }

    /// Settle-check for `JFLAG_NO_ONEARG`: set the bit iff all three 1-arg
    /// verdicts exist and none is alive. Called from the hook once per routed
    /// visit (after it filled all three) and from the deopt breaker after a
    /// kill. Any verdict still missing, or any variant alive → no bit (the
    /// serving/routing logic keeps consulting the maps).
    #[cfg(feature = "jit-native")]
    pub(crate) fn jit_maybe_mark_no_onearg(&mut self, proto_idx: usize) {
        let int_dead = match self.jit_native.get(&proto_idx) {
            Some(None) => true,
            Some(Some(np)) => np.dispatch_dead.get(),
            None => return,
        };
        let val_dead = match self.jit_value.get(&proto_idx) {
            Some(None) => true,
            Some(Some(_)) => false, // the value JIT has no deopt channel — never dies
            None => return,
        };
        let objp_dead = match self.jit_native_objparam.get(&proto_idx) {
            Some(None) => true,
            Some(Some(np)) => np.dispatch_dead.get(),
            None => return,
        };
        if int_dead && val_dead && objp_dead {
            self.jit_flags_set(proto_idx, JFLAG_NO_ONEARG);
        }
    }

    /// Deopt circuit-breaker: called by a dispatch serving site when a native
    /// run bailed (the interpreter re-runs the body right after, so this map
    /// bump is noise there). Once a (proto, family) has deopted
    /// `JIT_DEOPT_KILL` times, mark the proto dispatch-dead — serving it was
    /// pure per-call waste (a native attempt + a full interpreted re-run; the
    /// RuboCop walk's `line`/`matched` shapes deopt on 100% of calls). Deopt
    /// causes are overwhelmingly systematic (an unmodelled input type/shape),
    /// so a proto over the threshold essentially never wins later. The proto
    /// stays ALIVE in its map (its machine address may be baked into other
    /// compiled code's PIC caches — dropping it would free running code);
    /// `contains_key` stays true, so `jit_should_route`'s verdict logic is
    /// unchanged.
    #[cfg(feature = "jit-native")]
    fn jit_note_deopt(&mut self, proto_idx: usize, fam: u8) {
        const JIT_DEOPT_KILL: u32 = 32;
        let c = self.jit_deopt_count.entry((proto_idx, fam)).or_insert(0);
        *c += 1;
        if *c < JIT_DEOPT_KILL {
            return;
        }
        let np = match fam {
            0 => self.jit_native.get(&proto_idx),
            2 => self.jit_native_fparam.get(&proto_idx),
            3 => self.jit_native_objparam.get(&proto_idx),
            4 => self.jit_native_objparam2.get(&proto_idx),
            6 => self.jit_native_zeroarg.get(&proto_idx),
            _ => None,
        };
        if let Some(Some(np)) = np {
            np.dispatch_dead.set(true);
        }
        // A kill may settle the negative-cache bits.
        match fam {
            0 | 3 => self.jit_maybe_mark_no_onearg(proto_idx),
            4 => self.jit_flags_set(proto_idx, JFLAG_NO_OBJP2),
            6 => self.jit_flags_set(proto_idx, JFLAG_NO_ZEROARG),
            _ => {}
        }
    }

    /// Combined per-serve bookkeeping: stats (env-gated) + the deopt breaker
    /// (always on, deopt-path only).
    #[cfg(feature = "jit-native")]
    #[inline]
    pub(crate) fn jstat_serve(&mut self, proto_idx: usize, fam: u8, deopt: bool) {
        self.jstat_exec(proto_idx, fam, deopt);
        if deopt {
            self.jit_note_deopt(proto_idx, fam);
        }
    }

    /// Record a compile attempt outcome for a variant family. `pregated` =
    /// declined by the cheap pre-gate without running the full compiler.
    #[cfg(feature = "jit-native")]
    #[inline]
    pub(crate) fn jstat_compile(&mut self, fam: u8, ok: bool, pregated: bool) {
        if !self.jit_stats_on {
            return;
        }
        let c = &mut self.jit_stats.compile[fam as usize];
        c[0] += 1;
        if ok {
            c[1] += 1;
        }
        if pregated {
            c[2] += 1;
        }
    }

    /// Dump the block-frame phase profile (`RUBYRS_BLOCK_PROF`).
    pub(crate) fn dump_block_prof(&self) {
        if !self.block_prof_on {
            return;
        }
        let p = &self.block_prof;
        let total_inv = p.n[0] + p.n[1] + p.n[2];
        if total_inv == 0 {
            return;
        }
        // cntvct ticks → ns (24MHz ⇒ 41.67 ns/tick).
        let freq: u64;
        #[cfg(target_arch = "aarch64")]
        unsafe {
            std::arch::asm!("mrs {f}, cntfrq_el0", f = out(reg) freq);
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            freq = 1;
        }
        let ns = |ticks: u64| ticks as f64 * 1e9 / freq as f64;
        let per = |ticks: u64| ns(ticks) / total_inv as f64;
        eprintln!("== RUBYRS_BLOCK_PROF ==");
        eprintln!(
            "invocations ib1={} ib2={} general={} (fast→general fallbacks {})",
            p.n[0], p.n[1], p.n[2], p.n[3]
        );
        eprintln!("locals: share={} copy={} copy_slots_avg={:.1}",
            p.n_share, p.n_copy,
            if p.n_copy > 0 { p.copy_slots as f64 / p.n_copy as f64 } else { 0.0 });
        eprintln!("reent scan: frames_examined_avg={:.1}",
            if p.n_share + p.n_copy > 0 { p.reent_frames as f64 / (p.n_share + p.n_copy) as f64 } else { 0.0 });
        let names = ["snapshot", "gates", "argprep", "locals", "bind", "push", "  (reent)", "recycle(all frames)"];
        let mut tot = 0u64;
        for (i, nm) in names.iter().enumerate() {
            if i < 6 { tot += p.t[i]; }
            eprintln!("  {:<20} {:>10.1} ms total  {:>7.1} ns/inv", nm, ns(p.t[i]) / 1e6, per(p.t[i]));
        }
        eprintln!("  {:<20} {:>10.1} ms total  {:>7.1} ns/inv", "PROLOGUE SUM", ns(tot) / 1e6, per(tot));
        eprintln!("  recycle calls (all frame pops) = {}", p.n_recycle);
        eprintln!(
            "ib1/2 fallback reasons: rest_n0={} rest_n1p={} splat_arr={} splat_nonarr={} lambda_arity={} other={}",
            p.fb[0], p.fb[1], p.fb[2], p.fb[3], p.fb[4], p.fb[5]
        );
        eprintln!(
            "general shapes: autosplat={} rest_built={} kw_any={} blockparam={} plain_n0={} plain_n1={}",
            p.gshape[0], p.gshape[1], p.gshape[2], p.gshape[3], p.gshape[4], p.gshape[5]
        );
    }

    /// Dump the `RUBYRS_JIT_STATS` counters to stderr (called from
    /// `Runtime::drop`). Silent unless the env var is set.
    ///
    /// jit-native-gated like every field it reads: this cfg attr
    /// was previously orphaned onto `dump_block_prof` (the
    /// docblock-orientation bug class `lint-doc-orientation.sh`
    /// guards), which broke EVERY non-jit-native build target that
    /// compiles this fn (default-feature `cargo test --lib`,
    /// `clippy --all-targets`) with 29 missing-field errors.
    #[cfg(feature = "jit-native")]
    pub(crate) fn dump_jit_stats(&self) {
        if !self.jit_stats_on {
            return;
        }
        eprintln!("== RUBYRS_JIT_STATS ==");
        if self.rest_pred_stats != (0, 0) {
            eprintln!(
                "rest-pred serves={} declines={}",
                self.rest_pred_stats.0, self.rest_pred_stats.1
            );
        }
        if self.t2_compile_ns > 0 {
            eprintln!("tier2 compile time total={:.1}ms", self.t2_compile_ns as f64 / 1e6);
        }
        if self.t2_call_stats != [0; 3] {
            eprintln!(
                "tier2 t2_call ic_fast={} fallback={} native_native={}",
                self.t2_call_stats[0], self.t2_call_stats[1], self.t2_call_stats[2]
            );
        }
        if self.t2_block_stats != [0; 3] {
            eprintln!(
                "tier2 blocks invocations={} native_serves={} native_yield_serves={}",
                self.t2_block_stats[0], self.t2_block_stats[1], self.t2_block_stats[2]
            );
        }
        if self.t2_lite_blk_stats != [0; 2] {
            eprintln!(
                "tier2 lite_blk entries={} done={}",
                self.t2_lite_blk_stats[0], self.t2_lite_blk_stats[1]
            );
        }
        if self.symproc_serves != 0 {
            eprintln!("symproc direct serves={}", self.symproc_serves);
        }
        if self.restblk_census != [0; 6] {
            eprintln!(
                "restblk-census argc0={} argc1={} argc2={} argc3={} argc4={} argc5p={}",
                self.restblk_census[0], self.restblk_census[1], self.restblk_census[2],
                self.restblk_census[3], self.restblk_census[4], self.restblk_census[5]
            );
            let mut rows: Vec<(&usize, &u64)> = self.restblk_census_by.iter().collect();
            rows.sort_by(|a, b| b.1.cmp(a.1));
            for (pidx, n) in rows.into_iter().take(12) {
                eprintln!(
                    "  restblk {:<40} proto={} n={}",
                    self.protos.get(*pidx).map(|p| p.name.as_str()).unwrap_or("?"),
                    pidx, n
                );
            }
        }
        if self.t2_lite_stats != [0; 3] {
            eprintln!(
                "tier2 lite serves={} materialize_bails={} kills={}",
                self.t2_lite_stats[0], self.t2_lite_stats[1], self.t2_lite_stats[2]
            );
        }
        if self.t2_lite_call_stats != [0; 5] {
            eprintln!(
                "tier2 lite_call serves={} materializes={} chains={} cascade_frames={} const_serves={}",
                self.t2_lite_call_stats[0],
                self.t2_lite_call_stats[1],
                self.t2_lite_call_stats[2],
                self.t2_lite_call_stats[3],
                self.t2_lite_call_stats[4]
            );
        }
        for (i, name) in JIT_FAM_NAMES.iter().enumerate() {
            let [att, ok, pre] = self.jit_stats.compile[i];
            if att > 0 {
                eprintln!(
                    "compile {name:<9} attempts={att} ok={ok} pregate_declines={pre}"
                );
            }
        }
        let mut rows: Vec<_> = self.jit_stats.exec.iter().collect();
        rows.sort_by_key(|r| std::cmp::Reverse(r.1 .0));
        let total_calls: u64 = rows.iter().map(|r| r.1 .0).sum();
        let total_deopts: u64 = rows.iter().map(|r| r.1 .1).sum();
        eprintln!(
            "native-exec methods={} calls={} deopts={}",
            rows.len(),
            total_calls,
            total_deopts
        );
        for ((pidx, fam), (calls, deopts)) in rows.into_iter().take(60) {
            let name = self
                .protos
                .get(*pidx)
                .map(|p| p.name.as_str())
                .unwrap_or("?");
            eprintln!(
                "  exec {:<28} proto={:<6} fam={:<9} calls={} deopts={}",
                name, pidx, JIT_FAM_NAMES[*fam as usize], calls, deopts
            );
        }
    }




    /// Consume the in-flight non-local-return value, clearing any
    /// pending break/next transfer along with it.
    ///
    /// Invariant captured here: a `Op::ReturnMethod` that fires
    /// while a `begin/break` (or `next`) is mid-ensure walk
    /// supersedes that structured transfer (CRuby semantics —
    /// `return` wins, the break value is dropped). The
    /// `pending_loop_transfer` slot has to be cleared at the same
    /// instant `method_return` is consumed, otherwise an EndEnsure
    /// in a surviving frame could later resume into the now-stale
    /// target IP.
    ///
    /// All consume sites (currently `vm/step.rs::dispatch`'s unwind
    /// arm and `vm/kernel.rs::require_in_filescope`'s mimic of that
    /// unwind) must go through this helper rather than
    /// `self.method_return.take()` directly so the invariant cannot
    /// drift apart in one of them. Read-only `is_some()` checks
    /// keep using the field directly — they don't consume, so the
    /// invariant doesn't apply.
    /// Recompute the folded control-signal mask from the three
    /// underlying fields. MUST be called after every mutation of
    /// `method_return` / `break_signaled` / `pending_method_break`
    /// (see the `control_signals` field doc). Cheap enough that the
    /// cold mutation sites just call it unconditionally.
    #[inline]
    pub(crate) fn sync_control_signals(&mut self) {
        self.control_signals = (self.method_return.is_some() as u8)
            | ((self.break_signaled as u8) << 1)
            | (((!self.pending_method_breaks.is_empty()) as u8) << 2);
    }

    /// Debug-gate: does the cached mask agree with the fields?
    /// Asserted at the dispatch loop heads so a mutation site that
    /// forgot `sync_control_signals()` fails tests loudly.
    #[inline]
    pub(crate) fn control_signals_synced(&self) -> bool {
        self.control_signals
            == ((self.method_return.is_some() as u8)
                | ((self.break_signaled as u8) << 1)
                | (((!self.pending_method_breaks.is_empty()) as u8) << 2))
    }

    /// Cancel every pending loop-transfer / method-break entry
    /// suspended in a frame at index >= `frames_len` — i.e. in a
    /// frame that no longer exists once the stack has been popped
    /// or truncated to `frames_len` frames. Their ensure bodies
    /// were abandoned along with the frames, so their `EndEnsure`
    /// tails can never run; a stale survivor would either be
    /// mis-resumed by an unrelated later `EndEnsure` (coordinate
    /// collision with a recycled frame index) or leak its value.
    /// Call at every frame-pop/truncate site OUTSIDE the walks
    /// themselves (the walks — `unwind_with_exception`,
    /// `continue_method_break` — carry their own sweeps).
    /// Next value of the monotonic ensure-suspension counter —
    /// stamped into each [`SuspendCoord`] so `Op::EndEnsure` can
    /// order nested suspensions (innermost = highest seq).
    #[inline]
    pub(crate) fn next_suspend_seq(&mut self) -> u64 {
        self.suspend_seq += 1;
        self.suspend_seq
    }

    pub(crate) fn cancel_transfers_in_dead_frames(&mut self, frames_len: usize) {
        // Fast out: pending transfers are rare; keep the common
        // frame-pop paths (every plain `Op::Return`) to two loads.
        if self.pending_loop_transfers.is_empty() && self.pending_method_breaks.is_empty() {
            return;
        }
        self.pending_loop_transfers.retain(|t| {
            t.suspended.is_none_or(|s| s.frame_idx < frames_len)
        });
        self.pending_method_breaks.retain(|mb| {
            mb.suspended.is_none_or(|s| s.frame_idx < frames_len)
        });
        self.sync_control_signals();
    }

    pub(crate) fn take_method_return(&mut self) -> Option<Value> {
        let v = self.method_return.take();
        if v.is_some() {
            // A consumed non-local return supersedes any pending
            // transfer that is NOT parked inside an ensure body
            // (CRuby semantics — `return` wins, the break value is
            // dropped). SUSPENDED entries are left alone here: the
            // return may be entirely contained within a suspended
            // entry's ensure body (`def m; return 1; ensure; helper;
            // end` where helper's block returns), in which case that
            // entry must survive to resume at its EndEnsure. The
            // consumer that knows the return's target frame
            // (`begin_method_break`) cancels the suspended entries
            // the return actually escapes; the no-owner
            // LocalJumpError path cancels via
            // `unwind_with_exception`'s escape sweeps.
            self.pending_loop_transfers.retain(|t| t.suspended.is_some());
            self.pending_method_breaks.retain(|mb| mb.suspended.is_some());
        }
        self.sync_control_signals();
        // Always clear `method_return_locals` — the field-pair
        // invariant says it lives and dies with `method_return`,
        // and unconditional clear here is the cheapest way to
        // close the no-op-take leak window: a caller that takes
        // while `method_return` is already None (e.g. after
        // `clear_control_flow_signals` left a stale Rc behind in
        // some hypothetical future code path) still leaves the
        // VM in a consistent state. (code-review #285 round 2 #4.)
        self.method_return_locals = None;
        v
    }

    /// Consume the visibility-bypass flag set by `send` /
    /// `__send__` (and the `&nil` block-forward case). Returns
    /// whatever value the flag held and clears it to `false` in
    /// one step.
    ///
    /// The two existing consume sites (`vm/dispatch.rs::do_call`
    /// and `do_call_block` at the dispatch boundary) previously
    /// inlined `mem::replace(&mut self.bypass_visibility_once,
    /// false)`. The `take_*` named helper exists so a future
    /// dispatch-entry path can be added by grepping for `take_*`
    /// rather than knowing to spell out the `mem::replace` idiom
    /// from scratch — same discoverability win as
    /// `take_method_return`. The placement constraint the field's
    /// doc comment warns about (consume at dispatch boundary, NOT
    /// at the visibility-check site, otherwise the flag leaks
    /// when dispatch bottoms out before the Object arm) still
    /// applies regardless of which spelling you use; the helper
    /// doesn't enforce it.
    pub(crate) fn take_bypass_visibility(&mut self) -> bool {
        std::mem::replace(&mut self.bypass_visibility_once, false)
    }

    /// Consume the strict-public flag set by the `public_send`
    /// recogniser — the `take_bypass_visibility` twin (same
    /// consume-at-the-dispatch-boundary discipline; see the
    /// `require_public_once` field doc).
    pub(crate) fn take_require_public(&mut self) -> bool {
        std::mem::replace(&mut self.require_public_once, false)
    }

    /// Compute the maximum SymId still referenced by long-lived
    /// VM tables that must stay valid across `Runtime::reset` —
    /// `host_fns` (host-registered Ruby methods), and the two
    /// cext method tables. `Runtime::reset` uses this to floor
    /// the interner truncation so post-construction-registered
    /// names don't get their SymIds invalidated.
    ///
    /// `None` when all three tables are empty (no host or cext
    /// registrations) — caller treats this as "truncate to
    /// `snapshot.interner_len` unconditionally".
    ///
    /// Returns `usize` (the underlying repr of SymId) so the
    /// caller can do `keep_len = max(snapshot_len, this + 1)`
    /// directly. Walking all three tables on every reset is
    /// O(num_registered_methods); when cext libraries grow large,
    /// an incremental cache on each register-site would be
    /// cheaper — flagged as future work in PR #212's review.
    pub(crate) fn long_lived_sym_id_max(&self) -> Option<usize> {
        #[allow(unused_mut)]
        let mut max: Option<usize> = self
            .host_fns
            .keys()
            .map(|sym| sym.0 as usize)
            .max();
        // Both cext tables are themselves `#[cfg(feature = "cext")]`
        // (instance methods additionally `not(target_os = "wasi")`);
        // gate the walks the same way so this helper compiles
        // under `--no-default-features` — the fuzz crate
        // disables `cext` to keep the binary lean.
        #[cfg(feature = "cext")]
        for inner in self.cext_class_methods.values() {
            if let Some(m) = inner.keys().map(|sym| sym.0 as usize).max() {
                max = Some(max.map_or(m, |c| c.max(m)));
            }
        }
        #[cfg(all(feature = "cext", not(target_os = "wasi")))]
        for inner in self.cext_instance_methods.values() {
            if let Some(m) = inner.keys().map(|sym| sym.0 as usize).max() {
                max = Some(max.map_or(m, |c| c.max(m)));
            }
        }
        max
    }

    /// Reset every "control flow signal" flag — the per-call
    /// state Op handlers set to communicate break / return /
    /// loop-transfer / suppress-result / bypass-visibility
    /// requests across the dispatch loop. Called from both
    /// `Runtime::eval`'s entry (so a previous eval that left
    /// signals set doesn't bleed into the next) and
    /// `Runtime::reset` (same intent, different trigger). One
    /// helper means a future signal that's added to this set
    /// can't be missed at one site and present at the other —
    /// the kind of drift that's caused real bugs elsewhere in
    /// this codebase.
    pub(crate) fn clear_control_flow_signals(&mut self) {
        self.control_signals = 0;
        self.break_signaled = false;
        self.method_return = None;
        // Paired with `method_return` — see field doc. Without
        // this, a Runtime::reset between requests would leave a
        // stale Rc pinning the previous request's locals Vec
        // alive, AND silently violate the
        // `method_return.is_some() ⇔ method_return_locals.is_some()`
        // invariant. (code-review #285 round 2 #3.)
        self.method_return_locals = None;
        self.pending_loop_transfers.clear();
        self.pending_method_breaks.clear();
        self.suppress_call_result_push = false;
        self.bypass_visibility_once = false;
        self.require_public_once = false;
        // Boundary stack for AlreadyCaught propagation through
        // native iter drivers. Cleared here so a panic-aborted
        // dispatch_until (caught by Runtime::eval) doesn't leave
        // a stale entry that triggers spurious AlreadyCaught on
        // the next eval. See [`RubyError::AlreadyCaught`] doc.
        self.dispatch_until_depths.clear();
    }

    /// Vm-level inner half of `Runtime::reset_between_requests`.
    /// Clears the Vm-owned per-request transient state. The
    /// Runtime wrapper additionally handles the cext debug-
    /// assert (CURRENT_VM_PTR null) and the regex feature-
    /// gated last_match.
    ///
    /// Exposed for callers (the `_http_server` battery's
    /// per-request handler) that hold `&mut Vm` directly
    /// via `current_vm_ptr()` without going back through the
    /// Runtime API.
    #[cfg(feature = "_http_server")]
    pub(crate) fn reset_between_requests_inner(&mut self) {
        self.stack.clear();
        self.frames.clear();
        self.dm_share_depth = 0;
        self.locals_arena.clear();
        self.pinned.clear();
        self.class_stack.clear();
        self.class_visibility_stack.clear();
        self.module_function_active_stack.clear();
        self.globals.clear();
        self.clear_control_flow_signals();
        #[cfg(feature = "regex")]
        {
            self.last_match = None;
        }
    }

    /// Invalidate the inline constant caches. Call from EVERY site that
    /// mutates what a constant read can resolve to: `classes` /
    /// `constants` table inserts, per-class `consts` writes,
    /// `name_anon_class` re-homing, and include/prepend (they change
    /// the cref-ancestor constant walk). Cheap (one add) — definitions
    /// are rare next to reads.
    #[inline]
    pub(crate) fn bump_const_gen(&mut self) {
        self.const_gen = self.const_gen.wrapping_add(1);
    }

    pub(crate) fn collection_call(&mut self, recv: &Value, name: &str, args: &[Value]) -> Result<Option<Value>, Trap> {
        Ok(match recv {
            Value::Array(id) => return self.array_collection_call(*id, name, args),
            Value::Hash(id) => return self.hash_collection_call(*id, name, args),
            Value::Str(s) => return self.string_collection_call(s.clone(), name, args),
            Value::Range(id) => return self.range_collection_call(*id, name, args),
            // No-block Integer iterators return an Enumerator — `5.times
            // .map { }`, `1.upto(3).to_a`, `5.downto(1).each_slice(2)`.
            // The block forms live in collection_call_block; this hook
            // only fires on the no-block dispatch path, so the
            // Enumerator re-invokes the block form once driven.
            Value::Int(_)
                if matches!(
                    (name, args),
                    ("times", []) | ("upto", [_]) | ("downto", [_])
                        | ("step", [_]) | ("step", [_, _])
                ) =>
            {
                return self.make_enum_for(recv.clone(), name, args.to_vec()).map(Some);
            }
            // `1.5.step(to, by)` without a block → Enumerator too.
            Value::Float(_)
                if matches!((name, args), ("step", [_]) | ("step", [_, _])) =>
            {
                return self.make_enum_for(recv.clone(), name, args.to_vec()).map(Some);
            }
            // `nil.to_a` → `[]`, `nil.to_h` → `{}` (fresh each call).
            Value::Nil if matches!((name, args), ("to_a", []) | ("to_h", [])) => {
                self.maybe_gc();
                self.check_alloc()?;
                let v = if name == "to_a" {
                    Value::Array(self.heap.alloc(crate::heap::HeapObj::Array(Vec::new().into())))
                } else {
                    Value::Hash(self.heap.alloc(crate::heap::HeapObj::Hash(
                        crate::heap::HashObj::with_pairs(Vec::new()),
                    )))
                };
                return Ok(Some(v));
            }
            _ => None,
        })
    }



    /// Compare two values using built-in types first, then falling
    /// back to invoking the left-hand side's user-defined `<=>`.
    /// Returns `None` for incomparable pairs (built-in cross-type
    /// mismatches, or a user `<=>` that returns `nil`). Used by
    /// `Array#sort` so user classes that define `<=>` (typically
    /// via `include Comparable`) sort sensibly. Synchronously
    /// dispatches the user method by pushing a frame and running
    /// `dispatch_until` — the same pattern iterator drivers use.
    /// One step of nested lookup for `Hash#dig` / `Array#dig`.
    /// Hash receivers use `ruby_eq` key lookup; Array uses Int
    /// index (negative wraps from end). Anything else → nil so
    /// the caller can short-circuit cleanly.
    pub(crate) fn dig_step(&mut self, recv: &Value, key: &Value, allow_dispatch: bool) -> Result<Value, Trap> {
        // A NESTED value (allow_dispatch) that defines its own `dig`
        // — a Hash/Array SUBCLASS (class_tag) whose override re-converts
        // keys (Sinatra's IndifferentHash), or any user object with a
        // `dig` method (the Digger fixture) — is dispatched to, one key
        // at a time. The FIRST step (allow_dispatch = false) is the
        // receiver whose native `dig` is already running, so it must use
        // the internal lookup below — dispatching there would recurse
        // through the subclass override forever.
        if allow_dispatch {
            let dig_cls = match recv {
                Value::Hash(id) => self.heap.hash_class_tag(*id),
                Value::Array(id) => self.heap.array_class_tag(*id),
                Value::Object(oid) => Some(self.heap.class_of(*oid)),
                _ => None,
            };
            if let Some(cls) = dig_cls {
                let dig_id = self.interner.intern("dig");
                if let Some(m) = self.lookup_method_uncached(&cls, dig_id) {
                    let pre_frames = self.frames.len();
                    let mut g = PinGuard::new(self);
                    g.pin(recv.clone());
                    g.pin(key.clone());
                    g.vm.invoke_method(m, recv.clone(), vec![key.clone()])?;
                    g.vm.dispatch_until(pre_frames)?;
                    return Ok(g.vm.stack.pop().unwrap_or(Value::Nil));
                }
            }
        }
        match recv {
            Value::Hash(id) => {
                let id = *id;
                // Direct hit first — `vm_hash_find` so a key overriding
                // `hash`/`eql?` digs like CRuby (plain keys keep the
                // identity path).
                if let Some(p) = self.vm_hash_find(id, key)? {
                    return Ok(self.heap.hash(id)[p].1.clone());
                }
                // Missing key — CRuby's `Hash#dig` walks via `[]`
                // per step, which consults default_value first, then
                // default-block. Mirrors the `Hash#[]` missing-key
                // arm: scalar default returned as-is, block fired
                // with `(self_hash, key)` if no scalar default.
                if let Some(v) = self.heap.hash_default_value(id) {
                    return Ok(v);
                }
                if let Some(block_id) = self.heap.hash_default_block(id) {
                    let pre_frames = self.frames.len();
                    let mut g = PinGuard::new(self);
                    g.pin(Value::Hash(id));
                    g.pin(key.clone());
                    g.pin(Value::Block(block_id));
                    // Use the iter.rs step_block helper (#151);
                    // see `vm/hash.rs::Hash#[]` for the inline
                    // rationale on the BlockStep arms and why
                    // Break maps to LocalJumpError (stored Proc,
                    // not iterator yield).
                    match g.vm.step_block(block_id, vec![Value::Hash(id), key.clone()], pre_frames)? {
                        crate::vm::iter::BlockStep::MethodReturn => {
                            return Ok(Value::Nil);
                        }
                        crate::vm::iter::BlockStep::Break(_) => {
                            return Err(g.vm.trap(crate::error::RubyError::LocalJumpError {
                                msg: "break from proc-closure".into(),
                            }));
                        }
                        crate::vm::iter::BlockStep::Value(r) => {
                            return Ok(r);
                        }
                    }
                }
                Ok(Value::Nil)
            }
            Value::Array(id) => {
                if let Value::Int(i) = key {
                    let a = self.heap.array(*id);
                    let idx = if *i < 0 { a.len() as i64 + *i } else { *i };
                    Ok(a.get(idx as usize).cloned().unwrap_or(Value::Nil))
                } else {
                    Ok(Value::Nil)
                }
            }
            // `dig_step` is only ever reached with a key still to
            // consume, so a non-nil intermediate that isn't a Hash /
            // Array (and didn't dispatch its own `dig` above) is the
            // CRuby TypeError — `{"a"=>"1"}.dig("a", 1)` => "String does
            // not have #dig method" (spec_headers#test_dig).
            other => {
                let cname = match other {
                    Value::Object(oid) => self
                        .heap
                        .class_of(*oid)
                        .effective_name()
                        .unwrap_or_else(|| "Object".to_string()),
                    _ => crate::vm::numeric::class_name_for_error(other).to_string(),
                };
                Err(self.trap(crate::error::RubyError::TypeError {
                    msg: format!("{cname} does not have #dig method"),
                }))
            }
        }
    }

    pub(crate) fn user_cmp(&mut self, a: &Value, b: &Value) -> Result<Option<std::cmp::Ordering>, Trap> {
        // Heap-aware fast path so Array#sort works on BigInt
        // arrays — value_cmp_v alone would return None for any
        // BigInt operand and force fall-through to the user `<=>`
        // method dispatch, which doesn't exist for primitives.
        if let Some(ord) = value_cmp_v_heap(a, b, &self.interner, &self.heap) {
            return Ok(Some(ord));
        }
        // Try the receiver's `<=>` method (user-defined). A
        // `Value::Object` resolves it as an instance method; a
        // `Value::Class` resolves it as a class/singleton method —
        // `def self.<=>` walked via the metaclass chain (jekyll's
        // `Plugin.<=>` sorts plugin *classes* by priority, so
        // `klass_array.sort` compares Class receivers). Other
        // receiver types were already handled by value_cmp_v above.
        let spaceship = self.interner.intern("<=>");
        let method = match a {
            Value::Object(id) => {
                let cls = self.heap.class_of(*id);
                self.lookup_method_uncached(&cls, spaceship)
            }
            Value::Class(c) => self.lookup_class_singleton_method(c, spaceship),
            _ => None,
        };
        if let Some(m) = method {
            let pre_frames = self.frames.len();
            let mut g = PinGuard::new(self);
            g.pin(a.clone());
            g.pin(b.clone());
            g.vm.invoke_method(m, a.clone(), vec![b.clone()])?;
            g.vm.dispatch_until(pre_frames)?;
            let result = g.vm.stack.pop().unwrap_or(Value::Nil);
            drop(g);
            return Ok(match result {
                Value::Int(n) if n < 0 => Some(std::cmp::Ordering::Less),
                Value::Int(0) => Some(std::cmp::Ordering::Equal),
                Value::Int(_) => Some(std::cmp::Ordering::Greater),
                _ => None,
            });
        }
        Ok(None)
    }

    /// The `ArgumentError: comparison of X with Y failed` trap CRuby
    /// raises when a no-block `sort` / `sort!` / `min` / `max` reaches
    /// two elements with no usable `<=>` (returns nil, or none
    /// defined). `X` / `Y` are the operands' class names. Before this
    /// existed the no-block sort arms returned `Ok(None)` on an
    /// incomparable pair, which mis-surfaced as
    /// `NoMethodError: undefined method 'sort' for Array`.
    pub(crate) fn cmp_failed(&mut self, a: &Value, b: &Value) -> Trap {
        let an = match self.class_of(a) {
            Value::Class(c) => c.name.clone(),
            _ => a.type_name().to_string(),
        };
        let bn = match self.class_of(b) {
            Value::Class(c) => c.name.clone(),
            _ => b.type_name().to_string(),
        };
        self.trap(crate::error::RubyError::ArgumentError {
            msg: format!("comparison of {an} with {bn} failed"),
        })
    }

    /// Coerce `v` to a fresh Array for parallel assignment (`a, b = v`)
    /// and splat assignment (`a = *v`). CRuby treats the coerced RHS as a
    /// mutable work array, so an Array input is shallow-copied instead of
    /// reused. An object that responds to `to_ary` is converted (a
    /// non-Array `to_ary` result falls back to wrapping, leniently);
    /// anything else (`nil`, a scalar) becomes a one-element `[v]`.
    /// Backs `Op::MassignSplat`.
    pub(crate) fn massign_coerce_to_array(&mut self, v: Value) -> Result<Value, Trap> {
        if let Value::Array(id) = v {
            let elems = self.heap.array(id).clone();
            self.maybe_gc();
            self.check_alloc()?;
            return Ok(Value::Array(self.heap.alloc(crate::heap::HeapObj::Array(elems.into()))));
        }
        if let Value::Object(id) = &v {
            let cls = self.heap.class_of(*id);
            let to_ary = self.interner.intern("to_ary");
            if let Some(m) = self.lookup_method_uncached(&cls, to_ary) {
                let pre = self.frames.len();
                self.invoke_method(m, v.clone(), vec![])?;
                self.dispatch_until(pre)?;
                let r = self.stack.pop().unwrap_or(Value::Nil);
                if let Value::Array(id) = r {
                    let elems = self.heap.array(id).clone();
                    self.maybe_gc();
                    self.check_alloc()?;
                    return Ok(Value::Array(self.heap.alloc(crate::heap::HeapObj::Array(elems.into()))));
                }
            }
        }
        self.maybe_gc();
        self.check_alloc()?;
        Ok(Value::Array(self.heap.alloc(crate::heap::HeapObj::Array(vec![v].into()))))
    }

}


// cext-reentrance machinery (CURRENT_VM_PTR + VmPtrGuard + with_vm_ptr_set)
// moved to `vm/cext.rs`.
// `file_class_dispatch` moved to `vm/fileops.rs`.
//
// (The `with_caught_unwind` helper that used to live here was
// removed in Spike L3-A — once the cext call routes through the
// pure-C `rubyrs_jmp_invoke`, there's no Rust frame between
// setjmp and the cext fn that a catch_unwind could meaningfully
// cover. Re-introducing it on master appears to have been an
// accidental revert in the kernel.rs extraction refactor; this
// branch drops it again to keep the panic-budget honest and
// -D warnings green.)



/// Break the `Rc<Class>` reference cycles when the Vm goes away.
///
/// The class graph is cyclic BY DESIGN: `consts` tables hold
/// `Value::Class` edges (Object's table holds every top-level
/// class, including itself), `superclass` chains lead back into
/// those same classes, and `includes`/`prepends`/`singleton_view`
/// add more edges. `singleton_target` is already `Weak` (the one
/// back-edge someone broke at design time), but the rest keep
/// every class alive after the Vm drops — LeakSanitizer's
/// first-ever completed pass over the fuzz targets measured the
/// residue at a few KB per Runtime (41 objects on the parse
/// target). Harmless for the CLI's run-once-then-exit shape;
/// a slow leak for embedders that construct/drop many Runtimes.
///
/// On drop, walk every class reachable from the `classes` and
/// `constants` tables (and transitively through class `consts`)
/// and empty the cycle-bearing fields; the ordinary field drops
/// then free the whole graph. `try_borrow_mut` everywhere: a
/// Drop during panic-unwind may find a RefCell mid-borrow, and
/// leaking a little on that path beats a double-panic abort.
/// Zero panics — vm.rs has a panic budget of 0.
impl Drop for Vm {
    fn drop(&mut self) {
        // Collect reachable classes first (the clearing below
        // removes the edges we'd be traversing).
        let mut seen: std::collections::HashSet<usize> = std::collections::HashSet::new();
        let mut stack: Vec<Rc<Class>> = Vec::new();
        let mut all: Vec<Rc<Class>> = Vec::new();
        for c in self.classes.values() {
            stack.push(c.clone());
        }
        for v in self.constants.values() {
            if let Value::Class(c) = v {
                stack.push(c.clone());
            }
        }
        while let Some(c) = stack.pop() {
            if !seen.insert(Rc::as_ptr(&c) as usize) {
                continue;
            }
            if let Ok(consts) = c.consts.try_borrow() {
                for v in consts.values() {
                    if let Value::Class(inner) = v {
                        stack.push(inner.clone());
                    }
                }
            }
            if let Ok(sup) = c.superclass.try_borrow()
                && let Some(s) = sup.as_ref()
            {
                stack.push(s.clone());
            }
            if let Ok(sv) = c.singleton_view.try_borrow()
                && let Some(s) = sv.as_ref()
            {
                stack.push(s.clone());
            }
            all.push(c);
        }
        for c in &all {
            if let Ok(mut m) = c.methods.try_borrow_mut() {
                m.clear();
            }
            if let Ok(mut m) = c.singleton_methods.try_borrow_mut() {
                m.clear();
            }
            if let Ok(mut s) = c.superclass.try_borrow_mut() {
                *s = None;
            }
            if let Ok(mut i) = c.includes.try_borrow_mut() {
                i.clear();
            }
            if let Ok(mut p) = c.prepends.try_borrow_mut() {
                p.clear();
            }
            if let Ok(mut p) = c.singleton_prepends.try_borrow_mut() {
                p.clear();
            }
            if let Ok(mut i) = c.singleton_includes.try_borrow_mut() {
                i.clear();
            }
            if let Ok(mut v) = c.singleton_view.try_borrow_mut() {
                *v = None;
            }
            if let Ok(mut k) = c.consts.try_borrow_mut() {
                k.clear();
            }
            if let Ok(mut iv) = c.ivars.try_borrow_mut() {
                iv.clear();
            }
            if let Ok(mut cv) = c.class_vars.try_borrow_mut() {
                cv.clear();
            }
        }
    }
}
