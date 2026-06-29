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
mod lookup;
#[cfg(feature = "regex")]
mod match_data;
mod numeric;
mod primitive;
mod raise;
mod range;
mod sort;
mod sprintf;
mod step;
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
// _liquid_native / _sqlite) read it too — they're listed here so a
// `--no-default-features --features <accel>` build compiles. In that
// configuration the ptr is never set (the with_vm_ptr_set wrap lives
// on the cext dispatch path), so the host fns see null and decline
// to their pure-Ruby fallbacks — degraded but correct.
#[cfg(any(
    all(feature = "cext", not(target_os = "wasi")),
    feature = "_http_server",
    feature = "_fiber",
    feature = "_json_native",
    feature = "_yaml_native",
    feature = "_liquid_native",
    feature = "_sqlite",
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
    /// Vec cloned from the BlockHandle's `captured` at
    /// `invoke_block`. Holds the original `captured` Rc + the
    /// block's `param_start` so that, at Op::Return, the lower
    /// `[0..param_start]` portion of `locals` (the outer-scope
    /// slots — method-locals plus any enclosing block's slots) is
    /// COPIED BACK into the original Rc. This preserves
    /// closure-write-through to outer scope for the active
    /// invocation while keeping per-iteration isolation for slots
    /// that the block itself owns (params + body-locals).
    /// `None` for method / class-body / toplevel frames, and for
    /// block frames that don't need writeback (e.g. trivial
    /// invokes from non-iterating callers — currently unused, but
    /// the field allows future opt-in).
    pub(crate) block_writeback: Option<(Rc<RefCell<Vec<Value>>>, u16)>,
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
    pub(crate) rescues: Vec<RescueHandler>,
    pub(crate) loop_rescue_depths: Vec<usize>,
    pub(crate) loop_stack_depths: Vec<usize>,
    pub(crate) begin_rescue_depths: Vec<BeginBaseline>,
}

impl Frame {
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
/// One slot per VM is enough — break/next transfers are single-
/// frame and complete (or get superseded by a real raise)
/// before any new one can start.
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
    /// True while control is parked inside an `is_ensure` body
    /// because `continue_method_break` jumped to its handler IP.
    /// The dispatch loops' top-of-iteration check honours this:
    /// they skip firing `continue_method_break` while suspended
    /// so the ensure body runs to completion. `Op::EndEnsure`
    /// clears the flag before re-entering
    /// `continue_method_break`, resuming the walk.
    pub(crate) suspended: bool,
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
    /// Class filter for `rescue`. `None` means catch-all (used for
    /// `ensure` and as a future hook for internal/host-only handlers).
    /// `Some(Class(cls))` means the handler only fires when the raised
    /// exception's class is `cls` or a descendant. Bare `rescue` (no
    /// class listed) populates this with `StandardError`, so any
    /// exception that intentionally lives outside the StandardError
    /// subtree (e.g. `ResourceExhausted`) cannot be silently swallowed
    /// by `rescue => e`. Explicit `rescue ClassName => e` carries the
    /// resolved Class here. Multi-class clauses (`rescue A, B => e`)
    /// emit one handler per class — same handler_ip, same bind_slot —
    /// so each entry holds exactly one filter. `Some(Any(list))` is
    /// the splat form `rescue *CONST`: the constant's Array value is
    /// snapshotted into a class list at push time and the handler
    /// fires when ANY entry matches.
    pub(crate) filter_class: Option<RescueFilter>,
}

/// The two resolved shapes a `rescue` class filter can take. The
/// single-class form stays an `Rc` clone (no extra allocation on
/// the common path); the splat form carries the materialized list.
pub(crate) enum RescueFilter {
    /// `rescue Foo` / bare `rescue` (= StandardError) — one class.
    Class(Rc<Class>),
    /// `rescue *CONST` — match if any listed class matches. The
    /// list is the constant's Array value AS OF push time; CRuby
    /// re-evaluates the expression at match time, a divergence
    /// only observable if the array is mutated while the body
    /// runs (no real gem does this — minitest's
    /// PASSTHROUGH_EXCEPTIONS pattern is a frozen-ish constant).
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

pub(crate) struct Vm {
    pub(crate) protos: Vec<Proto>,
    #[cfg(feature = "jit-native")]
    pub(crate) jit_native: crate::intern::FxHashMap<usize, Option<crate::jit_native::NativeProto>>,
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
    /// Definition location (`file`, `line`) of each user-defined
    /// class/module/value constant, keyed by the same qualified-name
    /// `SymId` the `classes` / `constants` tables use. Recorded at
    /// `Op::DefClass` / `Op::DefModule` / `Op::StoreConst` (first
    /// definition wins, matching CRuby — reopens don't move it).
    /// Read by `Module#const_source_location`. `Rc<str>` filename is
    /// not a GC object, so no rooting. Snapshot/reset-managed like
    /// `constants` so embed resets don't leak user entries.
    pub(crate) const_source_locations: FxHashMap<SymId, (std::rc::Rc<str>, u32)>,
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
    pub(crate) autoload_paths: std::collections::HashMap<std::path::PathBuf, Vec<SymId>>,
    /// Per-call-site inline-cache counter. Each compiled `Op::Call`
    /// gets a unique u16 slot id; the Vm side allocates
    /// `call_caches[id]` lazily. Lives on the Vm so kernel
    /// builtins (e.g. `require_relative`) that compile new Ruby
    /// source at runtime can advance the counter without
    /// round-tripping through Runtime.
    pub(crate) cache_counter: u32,
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
    pub(crate) prim_reopen_mask: u8,
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
    /// In-flight `break`/`next` through `ensure` chain. Set by
    /// `Op::BreakLoop`/`Op::NextLoop` when an `is_ensure` handler
    /// sits between the source and the target; cleared once the
    /// transfer lands at its target loop label. `Op::EndEnsure`
    /// (emitted at the tail of every ensure handler body) reads
    /// this field to decide whether to keep walking the rescue
    /// chain or fall back to normal end-of-ensure exception
    /// re-raise. `unwind_with_exception` clears this field
    /// whenever a real exception starts unwinding — matching
    /// CRuby semantics where a `raise` inside an ensure body
    /// silently drops a pending break/next.
    pub(crate) pending_loop_transfer: Option<LoopTransfer>,
    /// ADR 0024 Phase A.4: in-flight block-break walking the
    /// yielding method's ensure chain before that frame returns.
    /// `Op::EndEnsure` checks this slot (after
    /// `pending_loop_transfer`) and calls `continue_method_break`
    /// to resume. Cleared once the yielding-method frame is
    /// popped and the break value lands on the caller's stack.
    pub(crate) pending_method_break: Option<MethodBreak>,
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
        // See the `class_singleton_deny` field doc. Union of every
        // name-keyed `do_call` arm that can fire for a Value::Class
        // receiver before the canonical user-singleton lookup, plus
        // the universal-Object names handled in the shared arms —
        // over-inclusion is harmless (slow path), under-inclusion is
        // a dispatch-precedence bug.
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
            jit_native_block_pred_float: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_floatcount_loop: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_floatfilter_loop: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_floatfind_loop: crate::intern::FxHashMap::default(),
            #[cfg(feature = "jit-native")]
            jit_native_on: std::env::var_os("RUBYRS_JIT_NATIVE").is_some(),
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
            cache_counter: 0,
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
            method_gen: 0,
            const_cache_flat: FxHashMap::default(),
            const_cache_chain: FxHashMap::default(),
            const_gen: 0,
            sym_length,
            class_singleton_deny,
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
            fast_index_checked_gen: 0,
            fast_index_hash_safe: false,
            fast_index_array_safe: false,
            fast_index_hash_set_safe: false,
            fast_index_array_set_safe: false,
            fast_index_hash_key_safe: false,
            fast_prim_str_safe: false,
            fast_prim_int_safe: false,
            any_undefs: false,
            prim_reopen_mask: 0,
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
            locals_arena: Vec::new(),
            control_signals: 0,
            pending_loop_transfer: None,
            pending_method_break: None,
            suppress_call_result_push: false,
            bypass_visibility_once: false,
            trailing_hash_positional: false,
            force_primitive_dispatch: false,
            pending_block_arg: None,
            #[cfg(feature = "_fiber")]
            fiber_yield_pending: None,
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
            | ((self.pending_method_break.is_some() as u8) << 2);
    }

    /// Debug-gate: does the cached mask agree with the fields?
    /// Asserted at the dispatch loop heads so a mutation site that
    /// forgot `sync_control_signals()` fails tests loudly.
    #[inline]
    pub(crate) fn control_signals_synced(&self) -> bool {
        self.control_signals
            == ((self.method_return.is_some() as u8)
                | ((self.break_signaled as u8) << 1)
                | ((self.pending_method_break.is_some() as u8) << 2))
    }

    pub(crate) fn take_method_return(&mut self) -> Option<Value> {
        let v = self.method_return.take();
        if v.is_some() {
            self.pending_loop_transfer = None;
            // Same invariant for the Phase A.4 block-break walk:
            // a return-from-block that fires mid-ensure-walk
            // supersedes the in-flight break (CRuby semantics —
            // `return` wins, the break value is dropped).
            self.pending_method_break = None;
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
        self.pending_loop_transfer = None;
        self.pending_method_break = None;
        self.suppress_call_result_push = false;
        self.bypass_visibility_once = false;
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
                // Direct hit first.
                {
                    let h = self.heap.hash(id);
                    if let Some(v) = h.iter()
                        .find(|(k, _)| k.ruby_eql(key, &self.heap))
                        .map(|(_, v)| v.clone())
                    {
                        return Ok(v);
                    }
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

    /// Coerce `v` to an Array for parallel assignment (`a, b = v`),
    /// CRuby-style: an Array stays as-is; an object that responds to
    /// `to_ary` is converted (a non-Array `to_ary` result falls back
    /// to wrapping, leniently); anything else (`nil`, a scalar)
    /// becomes a one-element `[v]`. Backs `Op::MassignSplat`.
    pub(crate) fn massign_coerce_to_array(&mut self, v: Value) -> Result<Value, Trap> {
        if matches!(v, Value::Array(_)) {
            return Ok(v);
        }
        if let Value::Object(id) = &v {
            let cls = self.heap.class_of(*id);
            let to_ary = self.interner.intern("to_ary");
            if let Some(m) = self.lookup_method_uncached(&cls, to_ary) {
                let pre = self.frames.len();
                self.invoke_method(m, v.clone(), vec![])?;
                self.dispatch_until(pre)?;
                let r = self.stack.pop().unwrap_or(Value::Nil);
                if matches!(r, Value::Array(_)) {
                    return Ok(r);
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