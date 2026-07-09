//! Tier-2 baseline JIT: the FRAME-KEEPING DIRECT-THREADED tier (ADR 0037).
//!
//! Compiles a method body's op SEQUENCE to native code that keeps the REAL
//! interpreter frame (locals, self, base_sp, ip) and the real operand stack:
//!
//! - branch targets become native jumps (no ip arithmetic, no re-fetch),
//! - op operands become immediates baked into specialized helper calls,
//! - (wave 2) the plain fixed-argc call ops run the IC-fast `t2_call` family
//!   and `Op::Return` runs the `t2_return` frame-pop shortcut,
//! - (wave 3) the hot simple ops INLINE against the pinned `Value` layout
//!   (ADR 0035): literals, `Locals::Stack` local reads/writes (with an SSA
//!   read-cache), small-Int arithmetic and comparisons, Sym/Int equality,
//!   `CaseEqLit`, ivar reads on an Object self, truthiness + fused
//!   compare-and-branch — no helper call on the fast path,
//! - every other admitted op runs through ONE generic helper that executes
//!   the interpreter's own `step()` for that op — so per-op semantics are
//!   the interpreter's by construction — and, when the op pushed a callee
//!   frame, drives it to completion with `dispatch_until` (the same nested-
//!   driver pattern the Rust iterator primitives use).
//!
//! THE correctness property, wave-3 form: the VM state (frame, operand
//! stack, `ip`) is EXACTLY the interpreter's state at every point FOREIGN
//! CODE CAN OBSERVE IT — i.e. before every runtime helper call (anything
//! that can raise, allocate/GC, call Ruby, or read frames — `binding`,
//! backtraces, the GC root walk), at every bail point, and on every branch
//! edge. BETWEEN those boundaries, values may live only in native registers:
//! a compile-time "virtual stack" holds not-yet-materialized operand-stack
//! entries, and a per-block read cache holds `Locals::Stack` slot values.
//! Every boundary MATERIALIZES the virtual stack (stores to the real
//! operand stack) first; local writes are WRITE-THROUGH (the canonical slot
//! is updated at the store op itself, with an inline drop-guard on the old
//! value), so the frame's locals are canonical at every instruction — the
//! cache is a pure read cache. This keeps the wave-1/2 property that a bail
//! is a mode switch, never a re-execution, and gives the GC zero native
//! surface: virtual values are restricted to tags with no destructor
//! obligations (no `Rc` payloads), and heap ids they may carry are always
//! also reachable from a rooted structure (the slot / stack cell they were
//! read from, which the read does not consume).
//!
//! Slow edges: a failed guard (unexpected tag, arithmetic overflow, a
//! revalidation-gated fast flag being off) materializes the virtual stack
//! and hands the REMAINING ops of the straight-line segment to `t2_resume`,
//! which executes them with the interpreter's own `step()` — including the
//! segment-ending branch — and reports where `ip` landed so the native code
//! can continue at the right block. No re-execution ever: guards run before
//! any effect of their op.
//!
//! Traps: a raising op (or a raising callee) propagates its `Trap` through
//! `Vm::t2_trap` and status 3; the serving site re-`Err`s it, and the outer
//! dispatch loop runs the exact rescue/unwind machinery it would have run for
//! the interpreted frame (our frame's `ip` is current at every raise point,
//! so backtrace spans are byte-identical).
//!
//! Admission declines only the ops that could redirect `ip` INTO this frame
//! behind the native code's back (rescue/ensure installation + raise/re-raise
//! terminators) and the non-local-exit ops whose owner semantics need the
//! master loop (`ReturnMethod`, block `Break`). Everything else — including
//! calls, blocks (`CreateBlock`/`CallBlock`/`Yield`), massign splats,
//! constants, globals — is admitted.

use std::mem::offset_of;

use cranelift_codegen::ir::{
    condcodes::IntCC, types, AbiParam, Block, BlockArg, InstBuilder, MemFlagsData, StackSlotData,
    StackSlotKind, Type, Value as ClValue,
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module};

use crate::bytecode::{BinOpKind, CaseLit, Op, Proto};
use crate::intern::SymId;
use crate::value::Value;

/// Native-run status codes returned by the compiled body / the generic op
/// helper. The serving site only distinguishes TRAP; DONE and BAIL both mean
/// "return to the dispatch loop, the VM state is consistent".
pub(crate) const T2_CONTINUE: i64 = 0;
/// The frame completed (popped by its `Return`, or consumed by an in-scope
/// non-local walk) — the result is on the operand stack.
pub(crate) const T2_DONE: i64 = 1;
/// Mid-body mode switch: the frame is intact with `ip` at the resume point
/// (a control signal or fiber yield is pending); the master loop continues.
pub(crate) const T2_BAIL: i64 = 2;
pub(crate) const T2_TRAP: i64 = 3;

/// Consecutive `t2_call` fast-probe declines at one call site before the
/// per-site settled-verdict byte short-circuits the probe (wave 3, item 3).
/// A settled site retries the probe roughly once per 1024 interpreter ops
/// (gated on `Vm::op_counter`), so a site whose receiver shape changes
/// (e.g. Str-heavy → Object-heavy) is re-discovered with bounded staleness.
const T2_SITE_SETTLE: u8 = 16;

/// A compiled tier-2 method body: `(vm) -> status`. Runs the frame currently
/// on top of `vm.frames` (which the serving site just pushed) to completion,
/// or bails/traps with the frame state consistent for the interpreter.
pub(crate) struct T2Proto {
    _module: JITModule,
    pub(crate) ptr: extern "C" fn(*mut crate::vm::Vm) -> i64,
    /// Wave-4 FRAME-LITE entry (`(vm, self_w0, self_w1, n_pop) -> status`)
    /// plus the baked plain-fixed argc, when the body passed the frame-lite
    /// admission (`t2_admit_lite`). Runs the body WITHOUT pushing a frame:
    /// the receiver/args stay on the operand stack (GC roots) for the whole
    /// run, locals live in a native spill slot, and any op the lite mode
    /// can't finish MATERIALIZES the real frame (the deferred push) and
    /// bails — a mode switch, never a re-execution. See `emit_body`'s lite
    /// entry + `t2_lite_materialize`.
    pub(crate) lite_ptr: Option<(T2LiteFn, u16)>,
    /// LITE-BLOCK entry (`(vm, self_w0, self_w1, block_id) -> status`) plus
    /// the baked `(param_start, n_params, is_rest)`, when the body passed
    /// the lite-block admission (`t2_admit_lite_block`). Runs a BLOCK body
    /// without pushing a block frame: the site pushes the bound arg(s) onto
    /// the operand stack (rooted), the own region (params + body locals)
    /// lives in a native spill slot, and captured-outer slot accesses
    /// (`< param_start`) route through the canonical binding cells via the
    /// `t2_lite_blk_outer_*` helpers — exactly where a frame's
    /// `outer_cell_for` routing would land them. Any unservable edge
    /// materializes the real BLOCK frame (share/copy decision + routing by
    /// the interpreter's own `block_frame_locals`) and BAILs.
    ///
    /// `is_rest` = the rest-only `|*a|` shape: the entry is compiled as a
    /// 1-param binder whose single "param" (slot `ps` == the handle's
    /// `rest_slot`) is the rest Array the serve site ALLOCATED BEFORE
    /// entering native state (the wave-4 no-GC-under-native invariant:
    /// the array is built while the interpreter still owns the world,
    /// pinned across a due collection, then rooted by its operand-stack
    /// slot for the whole frameless window).
    pub(crate) lite_blk_ptr: Option<(T2LiteBlkFn, u16, u16, bool)>,
}

/// Frame-lite entry ABI (wave 4): `(vm, self_w0, self_w1, n_pop) -> status`.
/// `self_w0/w1` are the raw words of the receiver (a BORROWING copy — the
/// serve site keeps the original rooted: on the operand stack for explicit
/// receivers, in the caller's frame / an owned Rust local for implicit
/// self). The callee's `argc` args are the top `argc` operand-stack slots
/// (left in place, still rooted); `n_pop` = argc (implicit self) or argc+1
/// (explicit receiver — the recv slot sits just below the args). On
/// `T2_DONE` the native code has replaced recv+args with the return value;
/// on `T2_BAIL` the real frame has been materialized (recv+args consumed,
/// `frame.ip` at the resume op) and the caller continues it exactly like
/// any freshly pushed frame.
pub(crate) type T2LiteFn = extern "C" fn(*mut crate::vm::Vm, i64, i64, i64) -> i64;

/// LITE-BLOCK entry ABI: `(vm, self_w0, self_w1, block_id) -> status`.
/// `self_w0/w1` borrow the BlockHandle's `self_val` (the handle stays live
/// for the whole frameless window — no GC can run); `block_id` is the
/// handle's heap id, used by the outer-slot helpers and the block-frame
/// materialize. The block's `n_params` bound args are the operand-stack top
/// (pushed by the serve site; `n_pop = n_params` is baked). Same DONE/BAIL
/// contract as `T2LiteFn`.
pub(crate) type T2LiteBlkFn = extern "C" fn(*mut crate::vm::Vm, i64, i64, i64) -> i64;

/// Which frameless variant `emit_body` is emitting (`Off` = the framed
/// tier-2 body).
#[derive(Clone, Copy)]
enum LiteMode {
    Off,
    /// Wave-4 method frame-lite: baked plain-fixed argc.
    Method(u16),
    /// Lite-block: baked `(param_start, n_params_bound, is_rest)`. The
    /// rest-only `|*a|` shape binds exactly like a 1-param block — its one
    /// "param" is the rest Array the serve site pre-allocates — so the
    /// flag only rides through to the serve-site guard tuple.
    Block(u16, u16, bool),
}

/// Compile-environment snapshot the serving site passes to `compile_tier2`
/// (values that live on the Vm and can't be reached from a bare `&Proto`).
pub(crate) struct T2Ctx {
    /// `RUBYRS_JIT_TIER2_NOCALL`: reproduce the wave-1 tier (every call op
    /// and `Return` through the generic helper). Implies `noinline`.
    pub(crate) nocall: bool,
    /// `RUBYRS_JIT_TIER2_NOINLINE`: reproduce the wave-2 tier (calls fast,
    /// but every simple op through its per-op helper; no inline lowering).
    pub(crate) noinline: bool,
    /// `RUBYRS_JIT_TIER2_NOLITE`: skip the wave-4 frame-lite emission and
    /// serving (reproduces the wave-3/5 tier) for controlled A/B.
    pub(crate) nolite: bool,
    /// Baked address of the Vm's `interrupt_pending` AtomicBool (the Arc's
    /// data; stable for the Vm's lifetime — the field is set once in the
    /// constructor). Read (relaxed) by the backward-branch poll gate.
    pub(crate) interrupt_addr: usize,
    /// `Vm::sym_nil_q` — the interned `nil?`, for the virtual-receiver
    /// `nil?` fusion at zero-arg call sites.
    pub(crate) sym_nil_q: u32,
}

// ---------------------------------------------------------------------------
// Pinned-layout facts the inline lowering relies on (ADR 0035): `Value` is
// `#[repr(u8)]`, 16 bytes — tag byte at offset 0, `bool` payload at offset 1,
// u32 payloads (SymId / ObjId) at offset 4, i64/f64/Rc payloads at offset 8.
// The codegen views a Value as two raw i64 words: w0 = bytes 0..8 (tag +
// bool + u32 payload), w1 = bytes 8..16.
// ---------------------------------------------------------------------------

/// Runtime discriminant byte of a `Value` (valid because of `#[repr(u8)]`).
#[inline]
fn tag_of(v: &Value) -> u8 {
    unsafe { *(v as *const Value as *const u8) }
}

/// The tag constants + masks the codegen bakes. Computed once at runtime from
/// sample values so cfg-dependent discriminant shifts can never desync.
pub(crate) struct T2Tags {
    int: u8,
    float: u8,
    sym: u8,
    bool_: u8,
    nil: u8,
    object: u8,
    /// Tags with NO destructor/refcount obligations (everything except the
    /// `Rc`-payload variants Str / Class / Regex and any variant we didn't
    /// enumerate): bit `1 << tag` set ⇒ a 16-byte copy is a legal clone and
    /// dropping a copy is a no-op. Heap-id variants (Object/Array/Hash/…)
    /// ARE in the mask — they're GC-managed, not refcounted.
    trivial_mask: u64,
    /// Tags for which the interpreter's `x.nil?` answer is served by
    /// `try_fast_primitive`'s universal arm (gated on `prim_reopen_mask`):
    /// Int / Float / Sym / Bool / Nil.
    nilq_mask: u64,
}

fn compute_tags() -> T2Tags {
    use crate::value::ObjId;
    let int = tag_of(&Value::Int(0));
    let float = tag_of(&Value::Float(0.0));
    let sym = tag_of(&Value::Sym(SymId(0)));
    let bool_ = tag_of(&Value::Bool(false));
    let nil = tag_of(&Value::Nil);
    let object = tag_of(&Value::Object(ObjId(0)));
    let mut trivial: Vec<u8> = vec![
        int,
        float,
        sym,
        bool_,
        nil,
        object,
        tag_of(&Value::Array(ObjId(0))),
        tag_of(&Value::Hash(ObjId(0))),
        tag_of(&Value::Range(ObjId(0))),
        tag_of(&Value::Block(ObjId(0))),
        tag_of(&Value::Rational(ObjId(0))),
        tag_of(&Value::BoundMethod(ObjId(0))),
        tag_of(&Value::UnboundMethod(ObjId(0))),
        tag_of(&Value::CurriedProc(ObjId(0))),
    ];
    #[cfg(feature = "bignum")]
    trivial.push(tag_of(&Value::BigInt(ObjId(0))));
    let mut trivial_mask = 0u64;
    for t in trivial {
        assert!(t < 64, "Value discriminant out of mask range");
        trivial_mask |= 1 << t;
    }
    let mut nilq_mask = 0u64;
    for t in [int, float, sym, bool_, nil] {
        nilq_mask |= 1 << t;
    }
    T2Tags { int, float, sym, bool_, nil, object, trivial_mask, nilq_mask }
}

/// Empirically probed field offsets of `Vec<Value>` (ptr / len / cap are all
/// word-sized; the ORDER is rustc-internal). Probed once with two distinct
/// vectors and cross-checked against `Vec<u64>`; a probe failure disables the
/// inline lowering (the tier falls back to wave-2 helper emission) rather
/// than miscompiling.
#[derive(Clone, Copy)]
pub(crate) struct VecLayout {
    ptr_off: i32,
    len_off: i32,
    cap_off: i32,
}

fn probe_one<T>(v: &Vec<T>, ptr: usize, len: usize, cap: usize) -> Option<(usize, usize, usize)> {
    if std::mem::size_of::<Vec<T>>() != 24 {
        return None;
    }
    let words: [usize; 3] = unsafe { *(v as *const Vec<T> as *const [usize; 3]) };
    let find = |x: usize| -> Option<usize> {
        let mut hit = None;
        for (i, w) in words.iter().enumerate() {
            if *w == x {
                if hit.is_some() {
                    return None; // ambiguous
                }
                hit = Some(i * 8);
            }
        }
        hit
    };
    let (p, l, c) = (find(ptr)?, find(len)?, find(cap)?);
    if p == l || l == c || p == c {
        return None;
    }
    Some((p, l, c))
}

fn probe_vec_layout() -> Option<VecLayout> {
    let mut v1: Vec<Value> = Vec::with_capacity(7);
    v1.push(Value::Int(1));
    v1.push(Value::Nil);
    v1.push(Value::Bool(true));
    let a = probe_one(&v1, v1.as_ptr() as usize, 3, v1.capacity())?;
    let mut v2: Vec<Value> = Vec::with_capacity(13);
    for i in 0..5 {
        v2.push(Value::Int(i));
    }
    let b = probe_one(&v2, v2.as_ptr() as usize, 5, v2.capacity())?;
    let mut v3: Vec<u64> = Vec::with_capacity(11);
    for i in 0..6 {
        v3.push(i);
    }
    let c = probe_one(&v3, v3.as_ptr() as usize, 6, v3.capacity())?;
    if a != b || b != c {
        return None;
    }
    Some(VecLayout { ptr_off: a.0 as i32, len_off: a.1 as i32, cap_off: a.2 as i32 })
}

fn vec_layout() -> Option<VecLayout> {
    static LAYOUT: std::sync::OnceLock<Option<VecLayout>> = std::sync::OnceLock::new();
    *LAYOUT.get_or_init(probe_vec_layout)
}

fn t2_tags() -> &'static T2Tags {
    static TAGS: std::sync::OnceLock<T2Tags> = std::sync::OnceLock::new();
    TAGS.get_or_init(compute_tags)
}

/// Read a `Value`'s raw 16 bytes as two i64 words (borrowing copy — the
/// original stays live/owned wherever it is).
#[inline]
fn value_words(v: &Value) -> [i64; 2] {
    unsafe { std::ptr::read_unaligned(v as *const Value as *const [i64; 2]) }
}

/// Rebuild an OWNED `Value` from raw words. Only sound for words whose tag
/// is in `trivial_mask` (no destructor/refcount obligations) or when the
/// caller genuinely transfers ownership of a non-trivial value.
#[inline]
unsafe fn value_from_words(w: [i64; 2]) -> Value {
    unsafe { std::mem::transmute::<[i64; 2], Value>(w) }
}

// ---------------------------------------------------------------------------
// Runtime helpers. All take `vm: *mut Vm`; the native code holds no Rust
// references, so reconstructing `&mut Vm` here is sound (same discipline as
// the jit_native primitives). GC safety: every Value lives in `vm.stack` /
// frame locals (real GC roots) at every point one of these helpers runs —
// the codegen materializes its virtual stack before ANY helper call.
// ---------------------------------------------------------------------------

/// Generic op executor: set `frame.ip` past the op (spans/backtraces/resume
/// all key off it), run the interpreter's own `step`, drive any pushed callee
/// frames to completion, then report signals. Mirrors one iteration of
/// `dispatch_until_inner`'s body with the fetch/decode replaced by baked
/// `pidx`/`ip` immediates.
/// `op` is a baked pointer INTO the proto's `code` buffer — stable because
/// protos are append-only (`Vec<Proto>` growth moves the Proto struct, not
/// its `code` heap buffer) and never mutated after construction.
unsafe extern "C" fn t2_op(vm: *mut crate::vm::Vm, op: *const Op, pidx: i64, ip: i64) -> i64 {
    let vm = unsafe { &mut *vm };
    let (pidx, ip) = (pidx as usize, ip as usize);
    let depth = vm.frames.len();
    {
        let f = vm.frames.last_mut().expect("ICE: t2_op with empty frame stack");
        f.ip = ip + 1;
    }
    let op = unsafe { *op };
    // TEMPORARY census (`RUBYRS_T2_FALLBACK_STATS=1`): every op a
    // compiled body still runs through the generic helper, keyed by
    // op tag (+ call name for the call family). One `is_some` branch
    // when disabled.
    let census = vm.t2_op_stats.is_some();
    if census {
        t2_census_note_op(vm, &op);
    }
    let r = vm.step(op, pidx);
    if census {
        // Clear an untaken call marker (the op's arm may not route
        // through the do_call family at all).
        vm.t2_fb_from = false;
    }
    if let Err(t) = r {
        vm.t2_trap = Some(t);
        return T2_TRAP;
    }
    t2_finish(vm, depth)
}

/// TEMPORARY census helper (`RUBYRS_T2_FALLBACK_STATS=1`): record a
/// generic-helper op execution and, for the call family, set the
/// one-shot marker so the dispatch the op's arm enters is tagged
/// t2-originating (see `Vm::t2_fb_from`).
#[cold]
fn t2_census_note_op(vm: &mut crate::vm::Vm, op: &Op) {
    use crate::bytecode::Op::*;
    let name = match op {
        Call(n, ..)
        | CallNoRecv(n, ..)
        | CallKw(n, ..)
        | CallKwNoRecv(n, ..)
        | ApplyCall(n, ..)
        | ApplyCallNoRecv(n, ..)
        | ApplyCallKw(n, ..)
        | ApplyCallKwNoRecv(n, ..)
        | ApplyCallKwBlock(n, ..)
        | ApplyCallKwNoRecvBlock(n, ..)
        | ApplyCallPrimitive(n, ..)
        | ApplyCallBlock(n, ..)
        | ApplyCallNoRecvBlock(n, ..)
        | Super(n, ..)
        | ApplySuper(n, ..)
        | ApplySuperBlock(n, ..)
        | CallBuiltinDirect(n)
        | CallBlock(n, ..)
        | CallNoRecvBlock(n, ..)
        | CallKwBlock(n, ..)
        | CallKwNoRecvBlock(n, ..)
        | CallAset(n, ..)
        | LoadLocalCall(_, n, ..) => Some(*n),
        _ => None,
    };
    if name.is_some() || matches!(op, InterpToS(_)) {
        vm.t2_fb_from = true;
    }
    let dbg = format!("{op:?}");
    let tag = dbg.split(['(', ' ']).next().unwrap_or("?").to_string();
    if let Some(m) = vm.t2_op_stats.as_mut() {
        *m.entry((tag, name)).or_insert(0) += 1;
    }
}

/// Shared post-op tail (`t2_op` and the wave-2 call helpers): drive any
/// pushed callee frame to completion with `dispatch_until`, then report the
/// exit statuses. (a) This frame is gone — a `Return` popped it, or an
/// in-scope non-local walk consumed it: DONE, the result is placed. (b) A
/// control signal is pending (non-local return / block break — the master
/// loop owns that walk), or (c) a fiber yield suspended us: BAIL with
/// `frame.ip` at the resume point, so interpretation continues seamlessly.
/// `depth` = `frames.len()` BEFORE the op ran.
#[inline]
fn t2_finish(vm: &mut crate::vm::Vm, depth: usize) -> i64 {
    if vm.frames.len() > depth
        && let Err(t) = vm.dispatch_until(depth)
    {
        vm.t2_trap = Some(t);
        return T2_TRAP;
    }
    if vm.frames.len() < depth {
        return T2_DONE;
    }
    if vm.control_signals != 0 {
        return T2_BAIL;
    }
    #[cfg(feature = "_fiber")]
    if vm.fiber_yield_pending.is_some() {
        return T2_BAIL;
    }
    T2_CONTINUE
}

/// Straight-line interpreter resume (wave 3, the slow edge of every inline
/// guard): execute ops `[from, end)` with the interpreter's own `step`,
/// driving callee frames to completion — exactly what a chain of `t2_op`
/// calls would do. Ops before `end - 1` are straight-line by construction
/// (segments end at the first branch/leader); the LAST op may be the
/// segment-ending branch, whose `step` arm retargets `frame.ip` — the return
/// value carries the landing ip (high 32 bits) so the native caller can
/// dispatch to the right block. Low 8 bits: the usual status.
unsafe extern "C" fn t2_resume(vm: *mut crate::vm::Vm, pidx: i64, from: i64, end: i64) -> i64 {
    let vm = unsafe { &mut *vm };
    let (pidx, from, end) = (pidx as usize, from as usize, end as usize);
    let mut ip = from;
    let mut i = from;
    while i < end {
        let depth = vm.frames.len();
        vm.frames
            .last_mut()
            .expect("ICE: t2_resume with empty frame stack")
            .ip = i + 1;
        let op = vm.protos[pidx].code[i];
        if let Err(t) = vm.step(op, pidx) {
            vm.t2_trap = Some(t);
            return T2_TRAP;
        }
        let st = t2_finish(vm, depth);
        if st != T2_CONTINUE {
            return st;
        }
        i += 1;
        // The (possibly branch-retargeted) resume point. Reading it per
        // iteration keeps this correct even if a mid-segment op ever
        // retargeted ip (none do today — segments are straight-line).
        ip = vm.frames.last().map(|f| f.ip).unwrap_or(i);
    }
    T2_CONTINUE | ((ip as i64) << 32)
}

/// Entry-info fill (wave 3): `out[0]` = the frame's `Locals::Stack` arena
/// base slot index, or -1 for `Locals::Shared`; `out[1..3]` = the raw words
/// of `frame.self_val` (a borrowing copy — the frame keeps it rooted for the
/// frame's whole lifetime, which outlives the native run).
unsafe extern "C" fn t2_entry_info(vm: *mut crate::vm::Vm, out: *mut i64) {
    let vm = unsafe { &mut *vm };
    let f = vm.frames.last().expect("ICE: t2_entry_info with empty frame stack");
    let base = match &f.locals {
        crate::vm::Locals::Stack(b) => *b as i64,
        crate::vm::Locals::Shared(_) => -1,
    };
    let sw = value_words(&f.self_val);
    unsafe {
        out.write(base);
        out.add(1).write(sw[0]);
        out.add(2).write(sw[1]);
    }
}

/// Ensure the operand stack has room for `extra` more slots (the cold edge
/// of the inline materialization's capacity check).
unsafe extern "C" fn t2_stack_reserve(vm: *mut crate::vm::Vm, extra: i64) {
    let vm = unsafe { &mut *vm };
    vm.stack.reserve(extra as usize);
}

/// Backward-branch poll (wave 3): fires only when the inline gate saw a
/// nonzero byte (control signal / SIGINT / fuel-or-deadline active). Sets
/// `ip` to the branch target (a canonical op boundary) so a BAIL resumes
/// exactly there; charges the fuel tick + deadline check `step()` would; a
/// pending signal or interrupt bails to the dispatch loop, whose loop head
/// owns the actual delivery machinery.
unsafe extern "C" fn t2_poll(vm: *mut crate::vm::Vm, ip: i64) -> i64 {
    let vm = unsafe { &mut *vm };
    vm.frames
        .last_mut()
        .expect("ICE: t2_poll with empty frame stack")
        .ip = ip as usize;
    if let Err(t) = vm.check_fuel() {
        vm.t2_trap = Some(t);
        return T2_TRAP;
    }
    if vm.control_signals != 0 {
        return T2_BAIL;
    }
    if vm
        .interrupt_pending
        .load(std::sync::atomic::Ordering::Relaxed)
    {
        return T2_BAIL;
    }
    #[cfg(feature = "_fiber")]
    if vm.fiber_yield_pending.is_some() {
        return T2_BAIL;
    }
    T2_CONTINUE
}

/// Lean `Op::LoadIvar` for a guard-checked `Value::Object` self (wave 3):
/// the codegen already extracted the oid, so this skips the frame fetch +
/// self clone + receiver match. Rides the interpreter's per-site ivar
/// cache (`cid`, ADR 0035 Ph4/5) — class-ptr guard → direct slot read;
/// holes/undefined read as Nil (CRuby). Returns 1 with the value's raw
/// words in `out` when the value is trivially-tagged (the caller keeps
/// it virtual); 0 when the value was pushed onto the real operand stack
/// (non-trivial tags need a real clone).
unsafe extern "C" fn t2_ivar_get(
    vm: *mut crate::vm::Vm,
    oid: i64,
    sym: i64,
    _cid: i64,
    out: *mut i64,
) -> i64 {
    let vm = unsafe { &mut *vm };
    let name_id = SymId(sym as u32);
    let id = crate::value::ObjId(oid as u32);
    let inst = vm.heap.instance(id);
    // Borrow-free shape scan (the class's names vec is hot for every
    // instance) — measured FASTER here than the per-site cid cache,
    // whose cache-vector line is an extra memory touch per op.
    let v = match inst.class.ivar_slot_lookup_fast(name_id) {
        Some(slot) => inst.ivars.read_slot_raw(slot),
        None => Value::Nil,
    };
    if t2_tags().trivial_mask & (1u64 << tag_of(&v)) != 0 {
        let w = value_words(&v);
        // `v` is a clone; trivially-tagged values have no Drop side
        // effects, so handing the words over is ownership-clean.
        std::mem::forget(v);
        unsafe {
            out.write(w[0]);
            out.add(1).write(w[1]);
        }
        1
    } else {
        vm.stack.push(v);
        0
    }
}

/// `Op::StoreIvar` with the stored value passed in registers (a virtual,
/// trivially-tagged value — ownership transfers here). Mirrors the step
/// arm byte for byte, including the frozen guard (which can raise —
/// `ip` is stamped first so the FrozenError's backtrace line is the store
/// op's, exactly as `step()` would report it; found by the wave-4 acid
/// battery, latent since wave 3) and the per-site ivar slot cache
/// (`cid`, ADR 0035 Ph4/5).
unsafe extern "C" fn t2_ivar_set_v(
    vm: *mut crate::vm::Vm,
    sym: i64,
    _cid: i64,
    w0: i64,
    w1: i64,
    ip: i64,
) -> i64 {
    let vm = unsafe { &mut *vm };
    vm.frames
        .last_mut()
        .expect("ICE: t2_ivar_set_v with empty frame stack")
        .ip = ip as usize + 1;
    let name_id = SymId(sym as u32);
    let v = unsafe { value_from_words([w0, w1]) };
    let self_val = vm
        .frames
        .last()
        .expect("ICE: t2_ivar_set_v with empty frame stack")
        .self_val
        .clone();
    if let Err(t) = vm.frozen_ivar_guard(&self_val) {
        vm.t2_trap = Some(t);
        return T2_TRAP;
    }
    match &self_val {
        Value::Object(id) => {
            let inst = vm.heap.instance_mut(*id);
            // Writes must INTERN (first assignment defines the name);
            // the fast lookup covers the overwhelmingly common
            // already-known case without the site-cache line touch.
            let slot = match inst.class.ivar_slot_lookup_fast(name_id) {
                Some(s) => s,
                None => inst.class.ivar_slot_intern(name_id),
            };
            inst.ivars.write_slot(slot, v);
        }
        Value::Class(c) => {
            c.ivars.borrow_mut().insert(name_id, v);
        }
        Value::Hash(id) => {
            vm.heap.hash_ivar_set(*id, name_id, v);
        }
        Value::Array(id) => {
            vm.heap.array_ivar_set(*id, name_id, v);
        }
        Value::Str(s) => {
            let key = std::rc::Rc::as_ptr(s) as usize;
            let keep = s.clone();
            vm.str_ivars
                .entry(key)
                .or_insert_with(|| (keep, crate::intern::FxHashMap::default()))
                .1
                .insert(name_id, v);
            vm.any_str_ivars = true;
        }
        _ => { /* drop — mirrors the step arm */ }
    }
    T2_CONTINUE
}

/// Lean `Op::StoreIvar` serve for STACK-borne values (campaign P5a):
/// the stored value was materialized on the real operand stack (a
/// call-fed store, an interpolation result, or an `inline_on`-off
/// body) — the shapes that previously crossed the generic `t2_op`
/// boundary (op decode + `Vm::step` match + `t2_finish` frame probe;
/// the AM census's StoreIvar 55.7/iter row). Pops and runs the
/// interpreter arm's own body (`Vm::store_ivar_value` — the frozen
/// guard, the receiver-kind match, and the ADR 0035 Ph4/5 cid slot
/// cache, shared so the two cannot drift). The op cannot push a
/// frame and cannot GC-allocate on the success path, so no
/// `t2_finish`; `ip` is
/// stamped first so a FrozenError's backtrace line is the store
/// op's, exactly as `step()` would report it (the `t2_ivar_set_v`
/// contract).
unsafe extern "C" fn t2_store_ivar(vm: *mut crate::vm::Vm, sym: i64, cid: i64, ip: i64) -> i64 {
    let vm = unsafe { &mut *vm };
    vm.frames
        .last_mut()
        .expect("ICE: t2_store_ivar with empty frame stack")
        .ip = ip as usize + 1;
    let v = vm.stack.pop().expect("ICE: StoreIvar stack underflow");
    match vm.store_ivar_value(SymId(sym as u32), cid as u32, v) {
        Ok(()) => T2_CONTINUE,
        Err(t) => {
            vm.t2_trap = Some(t);
            T2_TRAP
        }
    }
}

/// Lean `Op::InterpToS` serve for a FRAMED tier-2 body (campaign P6b):
/// string-interpolation `to_s` conversion — previously the generic
/// `t2_op` boundary (op decode + `Vm::step` match; the AM census's
/// InterpToS row, 100% Symbol receivers — interpolated attribute
/// names). Mirrors the step arm exactly:
///
///  (1) A String receiver stays AS-IS — CRuby's `rb_obj_as_string`
///      returns a `T_STRING` unchanged and NEVER consults a user
///      `String#to_s` (CRuby-probed 3.4.8: `class String; def to_s;
///      "X"; end` leaves `"#{s}"` == `s`; a `String` subclass instance
///      interpolates to its own content). Pure no-op, no frame, no
///      `t2_finish`.
///  (2) Primitive `to_s` fast serve — Symbol / Integer, the same arms
///      `do_call`'s own fast buckets run, under `do_call`'s exact
///      gates: the `method_gen`-revalidated reopen flags
///      (`prim_reopen_mask` bit 3 for Symbol — a `class Symbol; def
///      to_s` reopen flips it, `to_s` being a universal arm name — and
///      `fast_prim_int_safe` for Integer) PLUS the refinement gate
///      `do_call` applies before those buckets. The Symbol conversion
///      is byte-identical to the `Symbol#to_s` walk bucket (US-ASCII
///      tag for an ascii-only name, else UTF-8); the Integer one is
///      `integer_to_s_value`, shared with `try_fast_primitive`. Both
///      produce an `Rc`-backed String (no GC-heap alloc → no
///      `maybe_gc`).
///  (3) Otherwise (a user `to_s`, a Float/Bool/Nil primitive, or a
///      reopened/refined Symbol|Integer) → DECLINE to the full
///      dispatch: exactly the step arm's `do_call(:to_s, 0)`, `ip`
///      stamped to `ip + 1` FIRST so a raising `to_s`'s backtrace line
///      matches `step()` / the interpreter loop (both stamp `ip + 1`
///      before running the op) and `t2_op`. A user `to_s` pushes a
///      callee frame, so `t2_finish` drives it — byte for byte the
///      machinery `t2_op` used for this op. LITE bodies keep their
///      materialize-bail via the emitter's default arm.
unsafe extern "C" fn t2_interp_to_s(vm: *mut crate::vm::Vm, cid: i64, ip: i64) -> i64 {
    let vm = unsafe { &mut *vm };
    // (1) String passthrough.
    if matches!(vm.stack.last(), Some(Value::Str(_))) {
        return T2_CONTINUE;
    }
    // (2) Primitive fast serve — under do_call's refinement + reopen
    // gates.
    let to_s = vm.sym_to_s;
    let refined = !vm.refined_method_names.is_empty() && vm.refined_method_names.contains(&to_s);
    if !refined {
        if vm.fast_index_checked_gen != vm.method_gen {
            vm.fast_index_revalidate();
        }
        match vm.stack.last() {
            Some(Value::Sym(s)) if vm.prim_reopen_mask & (1 << 3) == 0 => {
                let n = vm.interner.resolve(*s).to_string();
                let v = if n.is_ascii() {
                    Value::new_str_us_ascii(n)
                } else {
                    Value::new_str(n)
                };
                vm.stack.pop();
                vm.stack.push(v);
                return T2_CONTINUE;
            }
            Some(Value::Int(n)) if vm.fast_prim_int_safe => {
                let v = crate::vm::numeric::integer_to_s_value(*n);
                vm.stack.pop();
                vm.stack.push(v);
                return T2_CONTINUE;
            }
            _ => {}
        }
    }
    // (3) Decline to the full to_s dispatch (step arm), ip stamped
    // first, frame driven with t2_finish.
    vm.frames
        .last_mut()
        .expect("ICE: t2_interp_to_s with empty frame stack")
        .ip = ip as usize + 1;
    let depth = vm.frames.len();
    if let Err(t) = vm.do_call(to_s, 0, false, cid as u32) {
        vm.t2_trap = Some(t);
        return T2_TRAP;
    }
    t2_finish(vm, depth)
}

/// `Op::CaseEqLit` fast core, shared by the register-arg and stack-arg
/// variants. `kind`/`payload` describe the baked literal (0=Sym 1=Int
/// 2=Bool 3=Nil 4=Float). Mirrors the step arm's safe path exactly —
/// same revalidation, same refinement probe, same per-kind safety flag,
/// same `ruby_eq`. Returns 0/1 = the Bool answer, 2 = decline (the caller
/// re-runs the op through the interpreter).
fn t2_case_eq_core(vm: &mut crate::vm::Vm, kind: i64, payload: i64, arg: &Value) -> i64 {
    if vm.fast_index_checked_gen != vm.method_gen {
        vm.fast_index_revalidate();
    }
    let refined = !vm.refined_method_names.is_empty()
        && vm.refined_method_names.contains(&vm.sym_case_eq);
    if refined {
        return 2;
    }
    let (recv, safe) = match kind {
        0 => (Value::Sym(SymId(payload as u32)), vm.fast_case_eq_sym_safe),
        1 => (Value::Int(payload), vm.fast_case_eq_prim_safe),
        2 => (Value::Bool(payload != 0), vm.fast_case_eq_prim_safe),
        3 => (Value::Nil, vm.fast_case_eq_prim_safe),
        4 => (
            Value::Float(f64::from_bits(payload as u64)),
            vm.fast_case_eq_prim_safe,
        ),
        _ => return 2,
    };
    if !safe {
        return 2;
    }
    recv.ruby_eq(arg, &vm.heap) as i64
}

/// `Op::CaseEqLit` with the predicate value in registers (virtual, trivial
/// tag — a borrowing view; on decline the codegen re-materializes it).
unsafe extern "C" fn t2_case_eq_v(
    vm: *mut crate::vm::Vm,
    kind: i64,
    payload: i64,
    aw0: i64,
    aw1: i64,
) -> i64 {
    let vm = unsafe { &mut *vm };
    let arg = std::mem::ManuallyDrop::new(unsafe { value_from_words([aw0, aw1]) });
    t2_case_eq_core(vm, kind, payload, &arg)
}

/// `Op::CaseEqLit` with the predicate on the real operand stack: pops it
/// (with proper drop) on an answer; leaves it in place on decline.
unsafe extern "C" fn t2_case_eq_s(vm: *mut crate::vm::Vm, kind: i64, payload: i64) -> i64 {
    let vm = unsafe { &mut *vm };
    let arg = vm
        .stack
        .last()
        .cloned()
        .expect("ICE: t2_case_eq_s stack underflow");
    let r = t2_case_eq_core(vm, kind, payload, &arg);
    if r < 2 {
        vm.stack.pop();
    }
    r
}

// ---------------------------------------------------------------------------
// Wave-4 FRAME-LITE helpers (ADR 0037 wave 4). While a frame-lite activation
// is live, NO foreign code may observe the VM except these helpers — none of
// them reads `vm.frames`, raises, allocates a GC object, or calls Ruby, so
// the missing frame is unobservable and no GC can run while values sit in
// native state. Anything outside that envelope goes through
// `t2_lite_materialize` first (the deferred frame push) and then BAILs to
// the interpreter — a mode switch at an exact op boundary, never a
// re-execution.
// ---------------------------------------------------------------------------

/// Materialize the real frame for a frame-lite activation — the frame push
/// the serve site deferred, executed with the CURRENT native machine state:
///
/// - locals come from the native spill slot (`slot`, `n_locals` × 16 bytes).
///   Ownership accounting: a trivially-tagged word carries no obligations; a
///   NON-trivially-tagged word is necessarily the UNTOUCHED borrow of the
///   caller-supplied arg for that slot (the lite StoreLocal guards decline
///   any write over — or of — a non-trivial value), so transmuting it into
///   the arena TAKES the ownership that the forgotten stack slot below held.
/// - recv+args occupy stack slots `[trunc, trunc + n_pop)` (`trunc` = the
///   entry stack length minus `n_pop`); they are removed WITHOUT dropping —
///   every non-trivial arg's ownership just moved into the arena, trivial
///   args have nothing to drop, and the recv slot (if `n_pop > argc`)
///   transfers into `frame.self_val`. Operand-stack temporaries the body
///   flushed above them shift down and become the frame's operand entries
///   (`base_sp = trunc`).
/// - implicit-self entries (`n_pop == argc`) CLONE self from the borrowed
///   words (the caller's frame / an owned Rust local keeps the original).
///
/// The pushed frame is indistinguishable from the serve site's own push:
/// `defining_class` comes through the `Vm::t2_lite_dc` hand-off (set by the
/// serve site / the lite chain hand-off from the resolving `Method`, exactly
/// the value the framed push would stamp — `super`/cvar readers stay
/// unreachable because those ops decline lite admission, but `do_call`'s
/// Nil-self bare-call gates DO read it now that call ops are admitted).
/// Frame-capacity: the serve site ran `check_frames` BEFORE entering lite
/// (covering the innermost single push), and every lite→lite chain level
/// re-checked headroom for its cascade before deferring another frame — so
/// the pushes here are exactly the ones the interpreter would have done.
#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn t2_lite_materialize(
    vm: *mut crate::vm::Vm,
    pidx: i64,
    ip: i64,
    argc: i64,
    n_locals: i64,
    n_pop: i64,
    trunc: i64,
    slot: *const i64,
    self_w0: i64,
    self_w1: i64,
) {
    let vm = unsafe { &mut *vm };
    lite_materialize_core(
        vm,
        pidx as usize,
        ip as usize,
        argc as usize,
        n_locals as usize,
        n_pop as usize,
        trunc as usize,
        slot,
        self_w0,
        self_w1,
        0,
        0,
    );
}

/// LITE-BLOCK twin of `t2_lite_materialize`: the bail edge of a frameless
/// BLOCK body. No self words (the handle carries `self_val`); `blk` is the
/// BlockHandle id + 1, `ps` the own-region start.
#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn t2_lite_materialize_blk(
    vm: *mut crate::vm::Vm,
    pidx: i64,
    ip: i64,
    argc: i64,
    n_locals: i64,
    n_pop: i64,
    trunc: i64,
    slot: *const i64,
    blk: i64,
    ps: i64,
) {
    let vm = unsafe { &mut *vm };
    lite_materialize_core(
        vm,
        pidx as usize,
        ip as usize,
        argc as usize,
        n_locals as usize,
        n_pop as usize,
        trunc as usize,
        slot,
        0,
        0,
        blk,
        ps as usize,
    );
}

/// The materialize core (shared by the native bail edges and the lite call
/// helpers): drain any PENDING outer lite activations first — outermost
/// first, each frame's `trunc` adjusted for the recv+args slots the frames
/// below it removed — then push this activation's own deferred frame. The
/// outward-in cascade keeps `vm.frames` ordered exactly as the interpreter
/// would have built it.
#[allow(clippy::too_many_arguments)]
fn lite_materialize_core(
    vm: &mut crate::vm::Vm,
    pidx: usize,
    ip: usize,
    argc: usize,
    n_locals: usize,
    n_pop: usize,
    mut trunc: usize,
    slot: *const i64,
    self_w0: i64,
    self_w1: i64,
    blk: i64,
    ps: usize,
) {
    if !vm.t2_lite_pending.is_empty() {
        let recs = std::mem::take(&mut vm.t2_lite_pending);
        let mut removed = 0usize;
        for r in recs {
            unsafe {
                if r.blk != 0 {
                    push_lite_block_frame(
                        vm,
                        crate::value::ObjId((r.blk - 1) as u32),
                        r.pidx,
                        r.resume_ip,
                        r.n_locals,
                        r.n_pop,
                        r.trunc - removed,
                        r.slot,
                        r.ps,
                    );
                } else {
                    push_lite_frame(
                        vm,
                        r.pidx,
                        r.resume_ip,
                        r.argc,
                        r.n_locals,
                        r.n_pop,
                        r.trunc - removed,
                        r.slot,
                        r.self_w0,
                        r.self_w1,
                        r.dc,
                    );
                }
            }
            removed += r.n_pop;
            if vm.jit_stats_on {
                vm.t2_lite_call_stats[3] += 1;
            }
        }
        trunc -= removed;
    }
    let dc = vm.t2_lite_dc.take();
    unsafe {
        if blk != 0 {
            push_lite_block_frame(
                vm,
                crate::value::ObjId((blk - 1) as u32),
                pidx,
                ip,
                n_locals,
                n_pop,
                trunc,
                slot,
                ps,
            );
        } else {
            push_lite_frame(
                vm, pidx, ip, argc, n_locals, n_pop, trunc, slot, self_w0, self_w1, dc,
            );
        }
    }
    if vm.jit_stats_on {
        vm.t2_lite_stats[1] += 1;
    }
    // Breaker attribution: the proto whose SHAPE failed is the one that
    // materialized ITSELF — the suspended (cascade-drained) callers were
    // innocent bystanders and keep their streaks. (The serve-site bail
    // path and the chain-bail path deliberately do NOT count.)
    vm.t2_lite_note_bail(pidx);
}

/// Push ONE deferred lite frame (the wave-4 materialize body, parameterized
/// over `ip` and `defining_class` for the cascade drain). Ownership
/// accounting is the wave-4 contract verbatim (see `t2_lite_materialize`'s
/// doc): native local slots transmute into the arena, recv+args are removed
/// from the stack without dropping (their ownership transferred), an
/// explicit recv's words become `frame.self_val`, an implicit self clones
/// through the borrowed words.
#[allow(clippy::too_many_arguments)]
unsafe fn push_lite_frame(
    vm: &mut crate::vm::Vm,
    pidx: usize,
    ip: usize,
    argc: usize,
    n_locals: usize,
    n_pop: usize,
    trunc: usize,
    slot: *const i64,
    self_w0: i64,
    self_w1: i64,
    dc: Option<std::rc::Rc<crate::value::Class>>,
) {
    let arena_base = vm.locals_arena.len();
    vm.locals_arena.reserve(n_locals);
    for i in 0..n_locals {
        let w0 = unsafe { slot.add(i * 2).read() };
        let w1 = unsafe { slot.add(i * 2 + 1).read() };
        vm.locals_arena.push(unsafe { value_from_words([w0, w1]) });
    }
    // Remove recv+args [trunc, trunc + n_pop) WITHOUT dropping (ownership
    // transferred as documented above); temporaries above shift down.
    let l = vm.stack.len();
    debug_assert!(l >= trunc + n_pop, "ICE: lite materialize stack shape");
    unsafe {
        let p = vm.stack.as_mut_ptr();
        std::ptr::copy(p.add(trunc + n_pop), p.add(trunc), l - trunc - n_pop);
        vm.stack.set_len(l - n_pop);
    }
    let self_val = if n_pop > argc {
        // Explicit receiver: its (forgotten) stack slot's ownership
        // transfers here.
        unsafe { value_from_words([self_w0, self_w1]) }
    } else {
        // Implicit self: the caller's frame (or the serve site's owned
        // local) keeps the original — clone through a borrowing view.
        let b = std::mem::ManuallyDrop::new(unsafe { value_from_words([self_w0, self_w1]) });
        (*b).clone()
    };
    vm.frames.push(crate::vm::Frame {
        proto_idx: pidx,
        ip,
        locals: crate::vm::Locals::Stack(arena_base as u32),
        self_val,
        base_sp: trunc,
        is_class_body: false,
        swap_return: None,
        block_arg: None,
        defining_class: dc,
        lexical_cvar_class: None,
        #[cfg(feature = "regex")]
        saved_last_match: None,
        is_block: false,
        is_lambda: false,
        n_given_positional: argc as u16,
        kw_given_mask: 0,
        aux: None,
        pending_yield: false,
        block_writeback: None,
        dm_share: false,
        own_start: 0,
        outer_cell_start: 0,
        outer_cell: None,
        outer_rest: None,
        captured_yield_block: None,
    });
}

/// Push ONE deferred LITE-BLOCK frame — the block-frame push the serve site
/// deferred, executed with the current native state. The share/copy locals
/// decision, capture routing, and writeback identity all come from the
/// interpreter's own `block_frame_locals`, so the pushed frame is
/// indistinguishable from `invoke_block1`'s: the cell's own region
/// `[param_start, n_locals)` is overwritten from the native spill
/// (ownership accounting as in `push_lite_frame`: a non-trivial spill word
/// is the untouched borrow of the bound arg's stack slot, which is
/// forgotten below — outer slots `< param_start` were never spilled, their
/// reads/writes routed through the canonical cells all along).
#[allow(clippy::too_many_arguments)]
unsafe fn push_lite_block_frame(
    vm: &mut crate::vm::Vm,
    block_id: crate::value::ObjId,
    pidx: usize,
    ip: usize,
    n_locals: usize,
    n_pop: usize,
    trunc: usize,
    slot: *const i64,
    ps: usize,
) {
    let (captured, self_val, lexical_cvar_class, cim, cyb, is_lambda) = {
        let bh = vm.heap.block(block_id);
        (
            bh.captured.clone(),
            bh.self_val.clone(),
            bh.lexical_cvar_class.clone(),
            bh.captured_is_method_scope,
            bh.captured_yield_block,
            bh.is_lambda,
        )
    };
    let (cell, writeback, routing) =
        vm.block_frame_locals(&captured, pidx, n_locals, ps as u16, cim, block_id);
    {
        let mut locals = cell.borrow_mut();
        debug_assert!(locals.len() >= n_locals, "ICE: lite block cell size");
        for s in ps..n_locals {
            let w0 = unsafe { slot.add(s * 2).read() };
            let w1 = unsafe { slot.add(s * 2 + 1).read() };
            locals[s] = unsafe { value_from_words([w0, w1]) };
        }
    }
    // Remove the bound arg slots [trunc, trunc + n_pop) WITHOUT dropping
    // (their ownership moved into the cell above); flushed temporaries
    // above shift down and become the frame's operand entries.
    let l = vm.stack.len();
    debug_assert!(l >= trunc + n_pop, "ICE: lite block materialize stack shape");
    unsafe {
        let p = vm.stack.as_mut_ptr();
        std::ptr::copy(p.add(trunc + n_pop), p.add(trunc), l - trunc - n_pop);
        vm.stack.set_len(l - n_pop);
    }
    vm.push_block_frame(
        pidx,
        cell,
        self_val,
        lexical_cvar_class,
        is_lambda,
        writeback,
        routing,
        cyb,
    );
    let f = vm.frames.last_mut().expect("ICE: just pushed");
    f.ip = ip;
    f.base_sp = trunc;
}

/// Frame-lite `Op::Return` with the return value in registers (virtual,
/// trivially-tagged — ownership transfers here): discard recv/args/operand
/// temporaries (proper drops — `truncate` runs destructors) and place the
/// return value where the recv was. The frame pop / `$~`-`$!` restore /
/// locals-release/aux-recycle disciplines all vanish with the frame that
/// never existed (admission guarantees none of them could have observable
/// effects: no rescue/ensure, no `$~` writes, no aux, arena untouched).
unsafe extern "C" fn t2_lite_return_v(vm: *mut crate::vm::Vm, w0: i64, w1: i64, trunc: i64) -> i64 {
    let vm = unsafe { &mut *vm };
    let v = unsafe { value_from_words([w0, w1]) };
    vm.stack.truncate(trunc as usize);
    vm.stack.push(v);
    T2_DONE
}

/// Frame-lite `Op::Return` with the return value on the real operand stack
/// (a non-trivial temporary — e.g. a flushed helper result): pop it (owned),
/// truncate, push it back.
unsafe extern "C" fn t2_lite_return_s(vm: *mut crate::vm::Vm, trunc: i64) -> i64 {
    let vm = unsafe { &mut *vm };
    let v = vm.stack.pop().expect("ICE: lite return with empty stack");
    vm.stack.truncate(trunc as usize);
    vm.stack.push(v);
    T2_DONE
}

/// Frame-lite `Op::LoadIvar` on a guard-checked `Value::Object` self: like
/// `t2_ivar_get` but DECLINES (0) on a non-trivially-tagged value instead of
/// pushing it — the caller materializes the frame and the interpreter
/// re-runs the (effect-free so far) op. 1 = value words in `out`.
unsafe extern "C" fn t2_lite_ivar_get(
    vm: *mut crate::vm::Vm,
    oid: i64,
    sym: i64,
    out: *mut i64,
) -> i64 {
    let vm = unsafe { &mut *vm };
    let name_id = SymId(sym as u32);
    let id = crate::value::ObjId(oid as u32);
    let inst = vm.heap.instance(id);
    match inst.ivars.get(&inst.class, name_id) {
        Some(v) => {
            if t2_tags().trivial_mask & (1u64 << tag_of(v)) != 0 {
                let w = value_words(v);
                unsafe {
                    out.write(w[0]);
                    out.add(1).write(w[1]);
                }
                1
            } else {
                0
            }
        }
        None => {
            let w = value_words(&Value::Nil);
            unsafe {
                out.write(w[0]);
                out.add(1).write(w[1]);
            }
            1
        }
    }
}

/// Frame-lite `Op::StoreIvar` with both the stored value and self passed in
/// registers (`v` virtual/trivial — ownership transfers on success; self is
/// a borrowing view). Mirrors the step arm's per-receiver insert arms, but
/// DECLINES (returns 1) on a frozen receiver instead of raising — the caller
/// materializes and the interpreter re-runs the op, raising the canonical
/// FrozenError (with `inspect`) against the real frame. 0 = stored.
unsafe extern "C" fn t2_lite_ivar_set(
    vm: *mut crate::vm::Vm,
    sym: i64,
    w0: i64,
    w1: i64,
    self_w0: i64,
    self_w1: i64,
) -> i64 {
    let vm = unsafe { &mut *vm };
    let name_id = SymId(sym as u32);
    let v = unsafe { value_from_words([w0, w1]) };
    let sv = std::mem::ManuallyDrop::new(unsafe { value_from_words([self_w0, self_w1]) });
    // `frozen_ivar_guard`'s check, decline-instead-of-raise (its raise path
    // calls `inspect` — arbitrary Ruby — which must not run frameless).
    let frozen = match &*sv {
        Value::Object(id) => vm.heap.instance(*id).frozen.get(),
        Value::Hash(id) => vm.heap.hash_frozen(*id),
        Value::Array(id) => vm.heap.array_frozen(*id),
        _ => false,
    };
    if frozen {
        return 1; // v is trivially-tagged — discarding this copy is free
    }
    match &*sv {
        Value::Object(id) => {
            let inst = vm.heap.instance_mut(*id);
            let class = inst.class.clone();
            inst.ivars.insert(&class, name_id, v);
        }
        Value::Class(c) => {
            c.ivars.borrow_mut().insert(name_id, v);
        }
        Value::Hash(id) => {
            vm.heap.hash_ivar_set(*id, name_id, v);
        }
        Value::Array(id) => {
            vm.heap.array_ivar_set(*id, name_id, v);
        }
        Value::Str(s) => {
            let key = std::rc::Rc::as_ptr(s) as usize;
            let keep = s.clone();
            vm.str_ivars
                .entry(key)
                .or_insert_with(|| (keep, crate::intern::FxHashMap::default()))
                .1
                .insert(name_id, v);
            vm.any_str_ivars = true;
        }
        _ => { /* drop — mirrors the step arm */ }
    }
    0
}

/// Borrowing raw-words view of a `Value` for the frame-lite serve sites
/// (vm.rs) — the original stays owned/rooted wherever it lives.
#[inline]
pub(crate) fn lite_self_words(v: &Value) -> [i64; 2] {
    value_words(v)
}

/// LITE-BLOCK captured-outer slot READ (`slot < param_start`): route to the
/// canonical binding cell — the handle's `captured` for
/// `slot >= creator_start`, the ancestor chain below that (exactly where a
/// frame's `outer_cell_for` routing lands, for BOTH the share-direct and
/// copy shapes: `captured` is canonical for its region in each). The value
/// is CLONED onto the real operand stack (an `Rc`/id copy — no GC
/// allocation, no raise; a missing slot reads Nil like the interpreter's
/// defensive arm). Returns 0 (always serves).
unsafe extern "C" fn t2_lite_blk_outer_get(
    vm: *mut crate::vm::Vm,
    blkid: i64,
    slot: i64,
) -> i64 {
    let vm = unsafe { &mut *vm };
    let slot = slot as usize;
    let v = {
        let bh = vm.heap.block(crate::value::ObjId(blkid as u32));
        let cell = if slot >= bh.creator_start as usize {
            &bh.captured
        } else {
            match &bh.outer_chain {
                Some(chain) => crate::value::chain_owner_cell(chain, slot),
                None => &bh.captured, // creator_start == 0 by construction
            }
        };
        let b = cell.borrow();
        b.get(slot).cloned().unwrap_or(Value::Nil)
    };
    vm.stack.push(v);
    0
}

/// LITE-BLOCK captured-outer slot READ into registers (via `out`, a
/// scratch slot): a BORROWING word copy of the cell's current value, for
/// the fused-operand lowerings (BinOpLocalLocal) whose guards must run
/// with NO stack effect so a guard-fail can materialize at the op
/// boundary and let the interpreter re-run the whole fused op. The words
/// are only ever consumed under an Int-tag guard or discarded — never
/// treated as owned. Effect-free; always serves.
unsafe extern "C" fn t2_lite_blk_outer_read(
    vm: *mut crate::vm::Vm,
    blkid: i64,
    slot: i64,
    out: *mut i64,
) {
    let vm = unsafe { &mut *vm };
    let slot = slot as usize;
    let bh = vm.heap.block(crate::value::ObjId(blkid as u32));
    let cell = if slot >= bh.creator_start as usize {
        &bh.captured
    } else {
        match &bh.outer_chain {
            Some(chain) => crate::value::chain_owner_cell(chain, slot),
            None => &bh.captured,
        }
    };
    let b = cell.borrow();
    let w = match b.get(slot) {
        Some(v) => value_words(v),
        None => value_words(&Value::Nil),
    };
    unsafe {
        out.write(w[0]);
        out.add(1).write(w[1]);
    }
}

/// LITE-BLOCK captured-outer slot WRITE: pops the (owned) value off the
/// real operand stack and stores it through the same canonical-cell routing
/// as the read helper (`cell_store` grows the cell defensively — a Rust
/// alloc, no GC). Never fails.
unsafe extern "C" fn t2_lite_blk_outer_set(
    vm: *mut crate::vm::Vm,
    blkid: i64,
    slot: i64,
) {
    let vm = unsafe { &mut *vm };
    let slot = slot as usize;
    let v = vm.stack.pop().expect("ICE: lite blk outer set with empty stack");
    let cell = {
        let bh = vm.heap.block(crate::value::ObjId(blkid as u32));
        if slot >= bh.creator_start as usize {
            bh.captured.clone()
        } else {
            match &bh.outer_chain {
                Some(chain) => crate::value::chain_owner_cell(chain, slot).clone(),
                None => bh.captured.clone(),
            }
        }
    };
    crate::vm::cell_store(&cell, slot, v);
}

// ---------------------------------------------------------------------------
// LITE t2_call (ADR 0037 wave-4 follow-on): call ops inside FRAMELESS bodies.
//
// A `Call`/`CallNoRecv`/`LoadLocalCall` op in a lite body calls one of the
// `t2_lite_call_*` helpers below, which resolve through the SAME site IC the
// framed `t2_call` family uses and then either:
//
//   (a) SERVE the callee frameless — only families that are provably
//       frame-free, raise-free and GC-free while running: the getter fast
//       path, the jit_native NativeProto families (zeroarg / int / value /
//       objparam / fparam; compiled code never runs `maybe_gc` and never
//       touches `vm.frames`), the rest-predicate body-shape serve, the
//       cascade's own fast-prim/fast-index arms for non-Object receivers,
//       and — the prize — a lite→lite NATIVE CHAIN into a callee that is
//       itself lite-admitted; or
//
//   (b) MATERIALIZE the caller's frame (the wave-4 deferred push, `ip`
//       stamped AT the call op) and return 1 — the native body returns
//       `T2_BAIL` and the interpreter re-runs the call op against the real
//       frame: interpreted callees, IC misses, non-Public/builtin/closure
//       resolutions, wrong arity (the canonical ArgumentError comes from the
//       cascade), fuel/deadline-active runs, and every dispatch-boundary
//       state (`bypass_visibility_once` / `force_primitive_dispatch` /
//       refinements / singleton flags) all take this edge. A mode switch at
//       an exact op boundary, never a replay.
//
// CASCADING MATERIALIZATION (the lite→lite soundness core): before invoking
// a lite callee, the caller registers a `T2LitePending` record (its spill
// slot, stack shape, self words, `resume_ip = call op + 1`, and its
// `defining_class`). If the callee — or anything deeper — materializes,
// `lite_materialize_core` drains the pending records OUTERMOST-FIRST, so
// caller frames are pushed BELOW callee frames and `vm.frames` matches the
// interpreter's order exactly; each drained frame resumes interpreted after
// its call op, receiving the callee's return value at the exact stack
// position the interpreter would have left it. On a completed (DONE) chain
// the record is popped unused.
//
// The wave-4 invariant survives: while ANY activation is frameless, no
// foreign code observes the VM — every serve family above is frame-free and
// raise-free, and none can trigger a GC (`heap.alloc` never collects; only
// `maybe_gc` does, and no served path calls it), so values in native spill
// slots stay unreachable-but-immortal for the whole frameless window.
// ---------------------------------------------------------------------------

/// Caller-context view for the lite call/const helpers. The native entry of
/// a call-bearing lite body fills a 6-word stack slot
/// `[locals_slot_addr, trunc, n_pop, self_w0, self_w1, blk]` (the runtime
/// values; `blk` = the BlockHandle id + 1 for a LITE-BLOCK caller, 0 for a
/// method caller); `pidx`/`argc`/`n_locals`/`param_start` are compile-time
/// constants packed into the helper's `meta` immediate (low 32 = pidx,
/// bits 32..40 = n_locals, bits 40..44 = argc, bits 44..60 = param_start).
struct LiteCtx {
    slot: *const i64,
    trunc: usize,
    n_pop: usize,
    self_w0: i64,
    self_w1: i64,
    pidx: usize,
    argc: usize,
    n_locals: usize,
    /// BlockHandle id + 1 for a lite-BLOCK activation; 0 = method.
    blk: i64,
    /// The block's own-region start (0 for methods).
    ps: usize,
}

#[inline]
unsafe fn lite_ctx(ctx: *const i64, meta: i64) -> LiteCtx {
    let m = meta as u64;
    LiteCtx {
        slot: unsafe { ctx.read() } as usize as *const i64,
        trunc: unsafe { ctx.add(1).read() } as usize,
        n_pop: unsafe { ctx.add(2).read() } as usize,
        self_w0: unsafe { ctx.add(3).read() },
        self_w1: unsafe { ctx.add(4).read() },
        pidx: (m & 0xffff_ffff) as usize,
        n_locals: ((m >> 32) & 0xff) as usize,
        argc: ((m >> 40) & 0xf) as usize,
        blk: unsafe { ctx.add(5).read() },
        ps: ((m >> 44) & 0xffff) as usize,
    }
}

/// Materialize the CALLER's frame at op `ip` (conservative decline: the
/// interpreter re-runs the call op against the real frame). Returns the
/// helper's MATERIALIZED status. `reason`/`name`/`cargc` feed the
/// TEMPORARY `RUBYRS_T2_FALLBACK_STATS` census (reason codes 30..=44,
/// decode table on `Runtime::t2_fallback_stats_rows`); free when off.
#[inline]
fn lite_mat_here(
    vm: &mut crate::vm::Vm,
    c: &LiteCtx,
    ip: usize,
    reason: u8,
    name: SymId,
    cargc: usize,
) -> i64 {
    if vm.t2_fb_stats.is_some() {
        vm.t2_fb_record(reason, name, 15, cargc);
    }
    lite_materialize_core(
        vm, c.pidx, ip, c.argc, c.n_locals, c.n_pop, c.trunc, c.slot, c.self_w0, c.self_w1,
        c.blk, c.ps,
    );
    if vm.jit_stats_on {
        vm.t2_lite_call_stats[1] += 1;
    }
    1
}

/// Dispatch-boundary gates shared by every lite call form. Any hit →
/// materialize (the full cascade owns these states). `t2_poll_flags != 0`
/// (fuel or wall-clock deadline active) also declines: the per-call
/// `check_fuel` charge can raise, which must happen interpreted — the
/// re-run charges exactly what `step()` would.
#[inline]
fn lite_call_gates(vm: &crate::vm::Vm, name_id: SymId) -> bool {
    vm.t2_poll_flags == 0
        && !vm.bypass_visibility_once
        && !vm.force_primitive_dispatch
        && (vm.refined_method_names.is_empty() || !vm.refined_method_names.contains(&name_id))
}

/// Where the callee's receiver lives (drives both the serve placement and
/// the lite→lite operand-stack ABI).
enum LiteRecv {
    /// Explicit receiver ON the operand stack at this index
    /// (`[.., recv, a1..aN]`).
    Stack(usize),
    /// Implicit self: the caller's borrowed self words (rooted by the
    /// ultimate outer owner for the whole frameless window).
    SelfWords,
    /// `LoadLocalCall` fusion: the receiver lives in the caller's native
    /// spill slot — NOT on the stack. A lite→lite chain pushes a clone
    /// (the callee consumes the recv slot); every other serve reads it in
    /// place and only the RESULT is pushed (net effect = the fused op's).
    LocalSlot(*const Value),
}

/// Place a frameless serve's result: consume the callee's recv+args from
/// the operand stack and leave the result where the interpreter's call op
/// would have.
#[inline]
fn lite_place(vm: &mut crate::vm::Vm, recv: &LiteRecv, cargc: usize, v: Value) {
    match recv {
        LiteRecv::Stack(recv_idx) => {
            vm.stack[*recv_idx] = v;
            vm.stack.truncate(recv_idx + 1);
        }
        LiteRecv::SelfWords => {
            let keep = vm.stack.len() - cargc;
            vm.stack.truncate(keep);
            vm.stack.push(v);
        }
        LiteRecv::LocalSlot(_) => {
            vm.stack.push(v);
        }
    }
}

/// Serve an IC-resolved plain proto method (`m`: non-builtin, non-closure;
/// explicit forms additionally Public) frameless, or materialize. `oid`/`cls`
/// are `Some` for an Object receiver (`None` = the toplevel-main form, which
/// only the guard-free native families and lite→lite chains can serve).
#[allow(clippy::too_many_arguments)]
fn lite_serve_m(
    vm: &mut crate::vm::Vm,
    c: &LiteCtx,
    ip: usize,
    name_id: SymId,
    m: &std::rc::Rc<crate::value::Method>,
    cls: Option<&std::rc::Rc<crate::value::Class>>,
    oid: Option<crate::value::ObjId>,
    recv: LiteRecv,
    cargc: usize,
) -> i64 {
    let pidx = m.proto_idx;
    let fixed = match m.fixed_arity {
        Some(f) if f.required as usize == cargc => Some(f),
        // Wrong arity for a fixed method: the cascade raises the canonical
        // ArgumentError against the materialized frame.
        Some(_) => return lite_mat_here(vm, c, ip, 36, name_id, cargc),
        None => None,
    };
    if fixed.is_none() {
        // Non-fixed arity: the only frameless serve is the rest-predicate
        // body-shape fast path (frame-free, raise-free, alloc-free — see
        // `rest_pred_eval`'s exactness gates).
        if let Some(rid) = oid
            && let Some(rp) = vm.rest_pred_for(pidx)
            && let Some(split) = vm.stack.len().checked_sub(cargc)
            && let Some(result) = vm.rest_pred_eval(rp, pidx, rid, split, cargc)
        {
            if vm.jit_stats_on {
                vm.rest_pred_stats.0 += 1;
                vm.t2_lite_call_stats[0] += 1;
            }
            lite_place(vm, &recv, cargc, Value::Bool(result));
            return 0;
        }
        return lite_mat_here(vm, c, ip, 37, name_id, cargc);
    }
    // Trivial attr_reader: the frame-free getter read (both the explicit
    // and implicit dispatch paths serve this shape before anything else).
    if cargc == 0
        && let Some(rid) = oid
        && let Some(gsym) = vm.protos[pidx].getter_ivar
    {
        let v = vm.getter_ivar_read(rid, pidx, gsym);
        if vm.jit_stats_on {
            vm.t2_lite_call_stats[0] += 1;
        }
        lite_place(vm, &recv, cargc, v);
        return 0;
    }
    // jit_native NativeProto families (cache-hit-only: compilation and
    // routing stay on the interpreted paths — a miss materializes and the
    // cascade compiles as usual, so steady state converges). All families
    // are GC-free/frame-free by construction; a deopt returns `None` with
    // no observable effect, and the interpreted re-run is the established
    // deopt contract at every existing serve site.
    #[cfg(feature = "jit-native")]
    if vm.jit_native_on {
        let vm_ptr = vm as *const crate::vm::Vm;
        // A borrowed &Value view of the receiver for the native ABI.
        let sv_view = std::mem::ManuallyDrop::new(unsafe {
            value_from_words([c.self_w0, c.self_w1])
        });
        let recv_ref: &Value = match &recv {
            LiteRecv::Stack(i) => &vm.stack[*i],
            LiteRecv::SelfWords => &sv_view,
            LiteRecv::LocalSlot(p) => unsafe { &**p },
        };
        let cls_ptr = cls.map_or(0usize, |c| std::rc::Rc::as_ptr(c) as usize);
        let jflags = vm.jit_flags_get(pidx);
        if cargc == 0
            && jflags & crate::vm::JFLAG_NO_ZEROARG == 0
            && let Some(Some(np)) = vm.jit_native_zeroarg.get(&pidx)
        {
            let e = np.entry();
            if !e.dead && (e.guard_class == 0 || e.guard_class == cls_ptr) {
                let res = e.call(vm_ptr, recv_ref, 0);
                let boxed = res.map(|r| e.box_ret(r));
                let deopt = boxed.is_none();
                if let Some(boxed) = boxed {
                    vm.jstat_serve(pidx, 6, false);
                    if vm.jit_stats_on {
                        vm.t2_lite_call_stats[0] += 1;
                    }
                    lite_place(vm, &recv, cargc, boxed);
                    return 0;
                }
                debug_assert!(deopt);
                vm.jstat_serve(pidx, 6, true);
                return lite_mat_here(vm, c, ip, 38, name_id, cargc);
            }
        }
        if cargc == 1 && jflags & crate::vm::JFLAG_NO_ONEARG == 0 {
            // Value method (infallible, heap-read-only).
            if let Some(Some(vp)) = vm.jit_value.get(&pidx) {
                let top = vm.stack.len() - 1;
                let out = vp.call(vm_ptr, recv_ref, &vm.stack[top]);
                vm.jstat_exec(pidx, 5, false);
                if vm.jit_stats_on {
                    vm.t2_lite_call_stats[0] += 1;
                }
                lite_place(vm, &recv, cargc, out);
                return 0;
            }
            // Integer method (Int arg).
            if let Some(Some(np)) = vm.jit_native.get(&pidx) {
                let e = np.entry();
                if !e.dead
                    && (e.guard_class == 0 || e.guard_class == cls_ptr)
                    && let Some(x) = crate::jit_native::as_int(vm.stack.last().expect("lite call arg"))
                {
                    let res = e.call(vm_ptr, recv_ref, x);
                    let boxed = res.map(|r| e.box_ret(r));
                    if let Some(boxed) = boxed {
                        vm.jstat_serve(pidx, 0, false);
                        if vm.jit_stats_on {
                            vm.t2_lite_call_stats[0] += 1;
                        }
                        lite_place(vm, &recv, cargc, boxed);
                        return 0;
                    }
                    vm.jstat_serve(pidx, 0, true);
                    return lite_mat_here(vm, c, ip, 38, name_id, cargc);
                }
            }
            // Object arg → the objparam specialization.
            if matches!(vm.stack.last(), Some(Value::Object(_)))
                && let Some(Some(np)) = vm.jit_native_objparam.get(&pidx)
            {
                let e = np.entry();
                if !e.dead {
                    let top = vm.stack.len() - 1;
                    let arg_ptr = &vm.stack[top] as *const Value as i64;
                    let res = e.call(vm_ptr, recv_ref, arg_ptr);
                    let boxed = res.map(|r| e.box_ret(r));
                    if let Some(boxed) = boxed {
                        vm.jstat_serve(pidx, 3, false);
                        if vm.jit_stats_on {
                            vm.t2_lite_call_stats[0] += 1;
                        }
                        lite_place(vm, &recv, cargc, boxed);
                        return 0;
                    }
                    vm.jstat_serve(pidx, 3, true);
                    return lite_mat_here(vm, c, ip, 38, name_id, cargc);
                }
            }
        }
        // Float arg → the fparam specialization (outside the settle bit,
        // mirroring the framed sites).
        if cargc == 1
            && let Some(&Value::Float(f)) = vm.stack.last()
            && let Some(Some(np)) = vm.jit_native_fparam.get(&pidx)
        {
            let e = np.entry();
            if !e.dead {
                let res = e.call(vm_ptr, recv_ref, f.to_bits() as i64);
                let boxed = res.map(|r| e.box_ret(r));
                if let Some(boxed) = boxed {
                    vm.jstat_serve(pidx, 2, false);
                    if vm.jit_stats_on {
                        vm.t2_lite_call_stats[0] += 1;
                    }
                    lite_place(vm, &recv, cargc, boxed);
                    return 0;
                }
                vm.jstat_serve(pidx, 2, true);
                return lite_mat_here(vm, c, ip, 38, name_id, cargc);
            }
        }
    }
    // The prize: a lite→lite NATIVE CHAIN. The callee runs frameless
    // against the same operand stack; the caller suspends behind a pending
    // record so a deeper materialize cascades outward-in.
    if vm.jit_flags_get(pidx) & crate::vm::JFLAG_TIER2_LITE != 0
        && let Some(&Some((lf, la))) = vm.t2_lite_ptrs.get(pidx)
        && la as usize == cargc
    {
        // Rust-stack + frame-capacity headroom: a full cascade pushes
        // `pending + 2` frames (every suspended caller + this caller +
        // the callee's own materialize), which must stay within the
        // interpreter's frame cap; embedder caps (`max_frames`) decline
        // wholesale (rare, and the interpreted path enforces them
        // canonically).
        if vm.t2_depth >= crate::vm::T2_MAX_NATIVE_DEPTH
            || vm.frames.len() + vm.t2_lite_pending.len() + 2 > 10_000
            || vm.max_frames.is_some()
        {
            return lite_mat_here(vm, c, ip, 40, name_id, cargc);
        }
        let (n_pop, w) = match &recv {
            LiteRecv::Stack(recv_idx) => (cargc + 1, value_words(&vm.stack[*recv_idx])),
            LiteRecv::SelfWords => (cargc, [c.self_w0, c.self_w1]),
            LiteRecv::LocalSlot(p) => {
                // Push a CLONE of the local as the callee's stack recv (the
                // callee consumes its recv slot on DONE/materialize; the
                // caller's spill slot keeps its own copy untouched).
                let v = unsafe { (**p).clone() };
                vm.stack.push(v);
                let w = value_words(vm.stack.last().expect("just pushed"));
                (cargc + 1, w)
            }
        };
        // Suspend the caller: resume AFTER the call op (the call has
        // happened once the callee is entered), defining_class handed off.
        let caller_dc = vm.t2_lite_dc.take();
        vm.t2_lite_pending.push(crate::vm::T2LitePending {
            slot: c.slot,
            pidx: c.pidx,
            argc: c.argc,
            n_locals: c.n_locals,
            n_pop: c.n_pop,
            trunc: c.trunc,
            self_w0: c.self_w0,
            self_w1: c.self_w1,
            resume_ip: ip + 1,
            dc: caller_dc,
            blk: c.blk,
            ps: c.ps,
        });
        vm.t2_lite_dc = m.defining_class.as_ref().and_then(|w| w.upgrade());
        let pend_depth = vm.t2_lite_pending.len();
        vm.t2_depth += 1;
        let st = lf(vm as *mut crate::vm::Vm, w[0], w[1], n_pop as i64);
        vm.t2_depth -= 1;
        if st == T2_DONE {
            // Chain completed frameless: the record was never consumed —
            // pop it and restore the caller's defining_class hand-off.
            debug_assert_eq!(vm.t2_lite_pending.len(), pend_depth, "ICE: lite pending shape");
            let rec = vm.t2_lite_pending.pop().expect("ICE: lite pending record");
            vm.t2_lite_dc = rec.dc;
            if vm.jit_stats_on {
                vm.t2_lite_call_stats[2] += 1;
                vm.jstat_exec(pidx, 8, false);
            }
            if let Some(s) = vm.t2_lite_streak.get_mut(pidx) {
                *s = 0;
            }
            return 0;
        }
        debug_assert_eq!(st, T2_BAIL, "ICE: lite chain status");
        // The callee (or something deeper) materialized: the cascade
        // drained our record too — the caller's frame (below the
        // callee's) exists now. Breaker attribution happened at the
        // materialize itself (`lite_materialize_core`), so suspended
        // levels don't multiply one deep event into a kill streak.
        debug_assert!(vm.t2_lite_pending.len() < pend_depth, "ICE: lite bail without drain");
        if vm.jit_stats_on {
            vm.t2_lite_call_stats[1] += 1;
        }
        return 1;
    }
    lite_mat_here(vm, c, ip, 39, name_id, cargc)
}

/// `Op::Call(name, argc, cid)` in a FRAMELESS body — explicit receiver.
/// Stack (flushed): `[.., recv, a1..aN]`.
unsafe extern "C" fn t2_lite_call_ex(
    vm: *mut crate::vm::Vm,
    ctx: *const i64,
    meta: i64,
    name: i64,
    cargc: i64,
    cid: i64,
    ip: i64,
) -> i64 {
    let vm = unsafe { &mut *vm };
    let c = unsafe { lite_ctx(ctx, meta) };
    let (name_id, cargc, cid, ip) = (SymId(name as u32), cargc as usize, cid as u32, ip as usize);
    if !lite_call_gates(vm, name_id) {
        return lite_mat_here(vm, &c, ip, 30, name_id, cargc);
    }
    let Some(recv_idx) = vm.stack.len().checked_sub(cargc + 1) else {
        return lite_mat_here(vm, &c, ip, 31, name_id, cargc);
    };
    match &vm.stack[recv_idx] {
        Value::Object(oid) => {
            let oid = *oid;
            let Some(cls) = vm.heap.try_class_of(oid) else {
                return lite_mat_here(vm, &c, ip, 33, name_id, cargc);
            };
            let Some(m) = vm.lookup_method_cached(&cls, name_id, cid) else {
                return lite_mat_here(vm, &c, ip, 34, name_id, cargc);
            };
            if m.visibility.get() != crate::value::Visibility::Public
                || m.closure.is_some()
                || m.builtin.is_some()
            {
                return lite_mat_here(vm, &c, ip, 35, name_id, cargc);
            }
            lite_serve_m(vm, &c, ip, name_id, &m, Some(&cls), Some(oid), LiteRecv::Stack(recv_idx), cargc)
        }
        _ => {
            // Non-Object receiver: the cascade's own native arms
            // (fast-prim / fast-index) under `t2_call_impl`'s exact
            // singleton gates — both are frame-free, raise-free and
            // GC-heap-allocation-free (documented on their defs), and
            // both are no-ops on miss.
            let singleton_free = !vm.any_str_singletons
                && !vm.any_heap_singletons
                && !vm.any_hash_singletons
                && name_id != vm.sym_call;
            if singleton_free
                && (vm.try_fast_primitive(name_id, cargc, false)
                    || vm.try_fast_index(name_id, cargc, false))
            {
                if vm.jit_stats_on {
                    vm.t2_lite_call_stats[0] += 1;
                }
                return 0;
            }
            lite_mat_here(vm, &c, ip, 32, name_id, cargc)
        }
    }
}

/// `Op::CallNoRecv(name, argc, cid)` in a FRAMELESS body — implicit self
/// (from the entry's borrowed words). Mirrors `t2_call_impl`'s no_recv
/// composition: host-fn precedence, then the toplevel-method IC for a
/// main/Nil self, else the Object-self cached lookup (no visibility gate).
unsafe extern "C" fn t2_lite_call_ns(
    vm: *mut crate::vm::Vm,
    ctx: *const i64,
    meta: i64,
    name: i64,
    cargc: i64,
    cid: i64,
    ip: i64,
) -> i64 {
    let vm = unsafe { &mut *vm };
    let c = unsafe { lite_ctx(ctx, meta) };
    let (name_id, cargc, cid, ip) = (SymId(name as u32), cargc as usize, cid as u32, ip as usize);
    if !lite_call_gates(vm, name_id) {
        return lite_mat_here(vm, &c, ip, 30, name_id, cargc);
    }
    if vm.host_fns.contains_key(&name_id) {
        return lite_mat_here(vm, &c, ip, 42, name_id, cargc);
    }
    let sv = std::mem::ManuallyDrop::new(unsafe { value_from_words([c.self_w0, c.self_w1]) });
    if matches!(&*sv, Value::Nil) || vm.is_main_self(&sv) {
        // Toplevel bare call (`fib(n-1)` at main): the toplevel-method IC.
        // Only the guard-free serves apply (no receiver Object).
        let Some(m) = vm.lookup_toplevel_method_cache_hit(cid) else {
            return lite_mat_here(vm, &c, ip, 43, name_id, cargc);
        };
        if m.closure.is_some() || m.builtin.is_some() {
            return lite_mat_here(vm, &c, ip, 35, name_id, cargc);
        }
        return lite_serve_m(vm, &c, ip, name_id, &m, None, None, LiteRecv::SelfWords, cargc);
    }
    let Value::Object(oid) = &*sv else {
        return lite_mat_here(vm, &c, ip, 44, name_id, cargc);
    };
    let Some(cls) = vm.heap.try_class_of(*oid) else {
        return lite_mat_here(vm, &c, ip, 33, name_id, cargc);
    };
    let Some(m) = vm.lookup_method_cached(&cls, name_id, cid) else {
        return lite_mat_here(vm, &c, ip, 34, name_id, cargc);
    };
    // No visibility gate: implicit-self calls legally reach
    // private/protected methods.
    if m.closure.is_some() || m.builtin.is_some() {
        return lite_mat_here(vm, &c, ip, 35, name_id, cargc);
    }
    lite_serve_m(vm, &c, ip, name_id, &m, Some(&cls), Some(*oid), LiteRecv::SelfWords, cargc)
}

/// `Op::LoadLocalCall(slot, name, cid)` in a FRAMELESS body — the fused
/// zero-arg explicit-recv superinstruction. The receiver is read from the
/// caller's native spill slot IN PLACE (no push): a serve pushes only the
/// result, a lite→lite chain pushes a clone as the callee's stack recv, and
/// a decline materializes with NOTHING pushed so the interpreter re-runs
/// the whole fused op from scratch.
unsafe extern "C" fn t2_lite_call_local(
    vm: *mut crate::vm::Vm,
    ctx: *const i64,
    meta: i64,
    slot: i64,
    name: i64,
    cid: i64,
    ip: i64,
) -> i64 {
    let vm = unsafe { &mut *vm };
    let c = unsafe { lite_ctx(ctx, meta) };
    let (name_id, cid, ip) = (SymId(name as u32), cid as u32, ip as usize);
    if !lite_call_gates(vm, name_id) {
        return lite_mat_here(vm, &c, ip, 30, name_id, 0);
    }
    let recv_ptr = unsafe { c.slot.add(slot as usize * 2) } as *const Value;
    match unsafe { &*recv_ptr } {
        Value::Object(oid) => {
            let oid = *oid;
            let Some(cls) = vm.heap.try_class_of(oid) else {
                return lite_mat_here(vm, &c, ip, 33, name_id, 0);
            };
            let Some(m) = vm.lookup_method_cached(&cls, name_id, cid) else {
                return lite_mat_here(vm, &c, ip, 34, name_id, 0);
            };
            if m.visibility.get() != crate::value::Visibility::Public
                || m.closure.is_some()
                || m.builtin.is_some()
            {
                return lite_mat_here(vm, &c, ip, 35, name_id, 0);
            }
            lite_serve_m(vm, &c, ip, name_id, &m, Some(&cls), Some(oid), LiteRecv::LocalSlot(recv_ptr), 0)
        }
        _ => lite_mat_here(vm, &c, ip, 44, name_id, 0),
    }
}

/// `Op::LoadConstChain(chain_idx)` in a FRAMELESS body: serve the
/// interpreter's own inline constant cache (keyed `(proto, chain slot)`,
/// generation-tagged) — an IC hit clones the cached value onto the operand
/// stack (an `Rc`/heap-id copy; no GC allocation); a cold or invalidated
/// cache materializes and the interpreted arm resolves + refills as usual.
unsafe extern "C" fn t2_lite_const_chain(
    vm: *mut crate::vm::Vm,
    ctx: *const i64,
    meta: i64,
    chain_idx: i64,
    ip: i64,
) -> i64 {
    let vm = unsafe { &mut *vm };
    let c = unsafe { lite_ctx(ctx, meta) };
    let ip = ip as usize;
    let cache_key = (c.pidx as u32, chain_idx as u32);
    if let Some((v, g)) = vm.const_cache_chain.get(&cache_key)
        && *g == vm.const_gen
    {
        let v = v.clone();
        vm.stack.push(v);
        if vm.jit_stats_on {
            vm.t2_lite_call_stats[4] += 1;
        }
        return 0;
    }
    let census_name = if vm.t2_fb_stats.is_some() {
        vm.interner.intern("<const-chain>")
    } else {
        SymId(0)
    };
    lite_mat_here(vm, &c, ip, 41, census_name, 0)
}

/// `Op::LoadConst(sym)` in a FRAMELESS body: the flat per-SymId inline
/// constant cache (generation-tagged). The interpreter arm's
/// `private_constant` pre-check runs BEFORE its cache read, so a name in
/// `private_consts` declines here (the arm re-raises against the
/// materialized frame); a cold/invalidated slot materializes and the
/// interpreted arm resolves + refills (autoload, qualified-path walk,
/// const_missing, NameError — all against a real frame).
unsafe extern "C" fn t2_lite_const_flat(
    vm: *mut crate::vm::Vm,
    ctx: *const i64,
    meta: i64,
    sym: i64,
    ip: i64,
) -> i64 {
    let vm = unsafe { &mut *vm };
    let c = unsafe { lite_ctx(ctx, meta) };
    let (name_id, ip) = (SymId(sym as u32), ip as usize);
    if (vm.private_consts.is_empty() || !vm.private_consts.contains(&name_id))
        && let Some((v, g)) = vm.const_cache_flat.get(&name_id)
        && *g == vm.const_gen
    {
        let v = v.clone();
        vm.stack.push(v);
        if vm.jit_stats_on {
            vm.t2_lite_call_stats[4] += 1;
        }
        return 0;
    }
    lite_mat_here(vm, &c, ip, 41, name_id, 0)
}

/// `Op::LoadConstStr(sym)` — the fresh-string-literal push, shared by BOTH
/// tiers (frameless and framed): byte-for-byte the interpreter arm (fresh
/// `Value::Str` per execution — mutations must not alias, so there is no
/// cached-push shortcut — source-encoding retag, frozen_string_literal
/// stamp). Raise-free and GC-heap-free (`Value::Str` is `Rc`-backed), so
/// the lite tier serves it unconditionally and the framed tier skips the
/// generic `step()` round-trip. Like the other specialized simple ops, it
/// charges no fuel tick and stamps no `ip`.
unsafe extern "C" fn t2_push_const_str(vm: *mut crate::vm::Vm, sym: i64, pidx: i64) {
    let vm = unsafe { &mut *vm };
    let (id, pidx) = (SymId(sym as u32), pidx as usize);
    let s = vm.interner.resolve(id).clone();
    let v = Value::new_str(s.to_string());
    if let Some(enc) = vm.protos[pidx].source_encoding
        && let Value::Str(rs) = &v
    {
        vm.retag_literal_to_source_encoding(rs, enc);
    }
    if vm.protos[pidx].frozen_string_literal
        && let Value::Str(rs) = &v
    {
        rs.frozen.set(true);
    }
    vm.stack.push(v);
}

// ---------------------------------------------------------------------------
// Wave-2 call helpers (ADR 0037 wave 2): the IC-fast `t2_call` family + the
// `t2_return` frame-pop shortcut.
// ---------------------------------------------------------------------------

/// IC-fast call executor for the plain fixed-argc call ops inside a tier-2
/// body (`Op::Call` / `Op::CallNoRecv` / the `Op::LoadLocalCall` fusion).
/// Instead of re-entering the interpreter (`step` → `do_call`'s full arm
/// cascade), this front-loads the exact monomorphic fast path the cascade
/// would reach for the dominant receiver shape:
///
///   - explicit recv → `try_invoke_explicit_recv_cached` (IC-resolved public
///     fixed/NFA-plan method; serves the frameless NativeProto families —
///     int/value/objparam/objparam2/fparam/zeroarg — the frame-free getter
///     and rest-predicate serves, or pushes the frame and runs a compiled
///     callee via its trailing `t2_enter`: the native→native path),
///   - implicit self → `try_invoke_self_recv_cached` (same helper family, no
///     visibility gate — implicit-self calls legally reach private/protected
///     — with `host_fns` precedence preserved by the gate below).
///
/// SOUNDNESS (why skipping the cascade prefix is exact): both helpers serve
/// only `Value::Object` receivers, and every `do_call` arm that runs BEFORE
/// them is receiver-typed away from Object (the Str/Array/Block/Hash
/// singleton gates, `proc.call`, `try_fast_primitive`, `try_fast_index`) —
/// except the three dispatch-boundary states gated here
/// (`bypass_visibility_once`, `force_primitive_dispatch`, an active
/// refinement on this name), which fall back to the full path that consumes
/// them. ANY decline (`Ok(false)`) falls back to the interpreter's own op
/// arm — `trailing_hash_positional` set around a full `do_call`, byte for
/// byte `Op::Call`'s semantics — so misses re-resolve identically
/// (method_gen bumps after redefinition, megamorphic sites, non-Object
/// receivers, method_missing, visibility NoMethodErrors, arity errors).
///
/// Wave-3 item 3: a per-site settled-verdict byte (`Vm::t2_site_verdict`,
/// dense by cache_id) skips the fast probes at sites that chronically
/// decline (e.g. Str/Array-receiver sites whose serve lives in the cascade's
/// own arms) — a settled decline previously cost the probe prefix PLUS the
/// full cascade. The byte counts consecutive declines, resets on any fast
/// serve, and re-probes ~1/1024 calls so shape changes are re-discovered.
#[inline]
fn t2_call_impl(
    vm: &mut crate::vm::Vm,
    name_id: SymId,
    argc: usize,
    cache_id: u32,
    no_recv: bool,
) -> i64 {
    let depth = vm.frames.len();
    let mut fast = !vm.bypass_visibility_once
        && !vm.force_primitive_dispatch
        && (vm.refined_method_names.is_empty() || !vm.refined_method_names.contains(&name_id));
    let mut site_v: u8 = 0;
    if fast && cache_id != u32::MAX {
        site_v = vm
            .t2_site_verdict
            .get(cache_id as usize)
            .copied()
            .unwrap_or(0);
        if site_v >= T2_SITE_SETTLE && vm.op_counter & 1023 != 0 {
            fast = false;
        }
    }
    if fast {
        // The explicit-brace trailing-Hash-is-positional flag is live for
        // the whole dispatch under `Op::Call` (the class-singleton
        // closure branch reaches `invoke_method`'s binder, which consumes
        // it); set/cleared around the serve exactly like the interpreter
        // arm does around `do_call`.
        vm.trailing_hash_positional = true;
        // The `host_fns` probe mirrors `do_call`'s gate on BOTH no_recv fast
        // paths (a host-registered fn keeps precedence over a same-named
        // reachable method).
        let served = if no_recv {
            if vm.host_fns.contains_key(&name_id) {
                Ok(false)
            } else {
                let self_val = vm
                    .frames
                    .last()
                    .expect("ICE: t2_call(no_recv) with empty frames")
                    .self_val
                    .clone();
                if matches!(self_val, Value::Nil) || vm.is_main_self(&self_val) {
                    // Toplevel bare call (`fib(n-1)` at main) — do_call's
                    // FIRST block: the toplevel-method IC + stack-direct
                    // fixed invoke, mirrored condition for condition.
                    match vm.lookup_toplevel_method_cache_hit(cache_id) {
                        Some(m) => {
                            vm.try_invoke_fixed_method_from_stack(m, self_val, argc, None)
                        }
                        None => Ok(false),
                    }
                } else {
                    vm.try_invoke_self_recv_cached(name_id, argc, cache_id)
                }
            }
        } else {
            // `:call` is decided ONCE here and threaded through both the
            // P7 proc.call serve and `singleton_free` (which already needed
            // `name_id != sym_call`) — so the common non-`.call` explicit
            // recv pays exactly the one SymId compare it paid before P7.
            let is_call = name_id == vm.sym_call;
            // P7: `proc.call` / `lambda.call` on a `Value::Block` receiver —
            // do_call's fast proc.call arm (the AS callback-filter machinery:
            // `invoke_sequence.call` + the filter lambdas). Served in-body so
            // the tier-2 body stops falling back to the interpreter cascade
            // for it (census `prim-recv call Block`). Mirrors do_call's
            // ordering: the proc.call arm sits AFTER the per-instance
            // singleton gates (`any_heap_singletons` — a Block eigenclass
            // `def blk.call` would take precedence via that gate; when one
            // exists we fall back so the interpreter honours it) and BEFORE
            // `try_fast_primitive`. Arity (lambda-strict vs proc-lenient),
            // kwargs peel, `&blk`/splat binding, non-local return, and the
            // `break from proc-closure` LocalJumpError all live in the shared
            // `invoke_proc_call_body` helper — byte-identical to interp.
            if is_call
                && !vm.any_heap_singletons
                && vm
                    .stack
                    .len()
                    .checked_sub(argc + 1)
                    .is_some_and(|i| matches!(vm.stack.get(i), Some(Value::Block(_))))
            {
                let split = vm.stack.len() - argc;
                let args: Vec<Value> = vm.stack.drain(split..).collect();
                match vm.stack.pop() {
                    Some(Value::Block(bid)) => vm.invoke_proc_call_body(bid, args).map(|()| true),
                    _ => unreachable!("ICE: t2 proc.call recv vanished"),
                }
            } else {
                // Receiver-typed fast serves, in `do_call`'s exact order. The
                // Str/Array/Block/Hash per-instance singleton gates and the
                // `proc.call` arm sit BETWEEN the boundary gates and these
                // helpers in the cascade; the singleton gates are inert here
                // by the per-KIND guard (no singletons of those kinds exist)
                // and the `proc.call` arm was served just above (or fell to
                // this fallback when a Block eigenclass exists / the receiver
                // is not a Block), so skipping straight to the helpers is
                // exact — and every helper is a no-op on miss. `!is_call`
                // keeps a non-Block `:call` (e.g. an Object `def call`) off
                // `try_fast_primitive` (which never serves `:call`), matching
                // the cascade.
                let singleton_free = !vm.any_str_singletons
                    && !vm.any_heap_singletons
                    && !vm.any_hash_singletons
                    && !is_call;
                if singleton_free
                    && (vm.try_fast_primitive(name_id, argc, false)
                        || vm.try_fast_index(name_id, argc, false))
                {
                    Ok(true)
                } else {
                    // Object receiver → the explicit-recv monomorphic path;
                    // Class/Module receiver → the class-singleton sibling
                    // (each self-declines on receiver type, mirroring the
                    // cascade's ordering).
                    match vm.try_invoke_explicit_recv_cached(name_id, argc, cache_id) {
                        Ok(false) => {
                            vm.try_invoke_class_singleton_cached(name_id, argc, cache_id)
                        }
                        r => r,
                    }
                }
            }
        };
        // Mid-cascade WALK FAST BUCKETS (`Vm::try_walk_fast_buckets`, the
        // zone extracted from `do_call`): probed at the cascade's exact
        // position — right after the class-singleton sibling, right
        // before the slow cascade — so the in-body calls those buckets
        // serve (`===`, `is_a?`, `respond_to?`, Array/Hash size/empty?/
        // include?/push, send-family re-aims, …) stop paying the full
        // `do_call` preamble per call. Exactness: `fast` already covers
        // the boundary gates; the per-KIND singleton gate below mirrors
        // the str/heap/hash singleton arms that run BEFORE the zone in
        // `do_call` (no_recv zone arms are self/arg-based — no gate).
        let served = match served {
            Ok(false) => {
                let kind_free = no_recv
                    || match vm
                        .stack
                        .len()
                        .checked_sub(argc + 1)
                        .and_then(|i| vm.stack.get(i))
                    {
                        Some(Value::Str(_)) => !vm.any_str_singletons,
                        Some(Value::Array(_)) | Some(Value::Block(_)) => {
                            !vm.any_heap_singletons
                        }
                        Some(Value::Hash(_)) => !vm.any_hash_singletons,
                        _ => true,
                    };
                if kind_free {
                    vm.try_walk_fast_buckets(name_id, argc, no_recv, cache_id, false, false)
                } else {
                    Ok(false)
                }
            }
            r => r,
        };
        vm.trailing_hash_positional = false;
        match served {
            Ok(true) => {
                if site_v != 0
                    && cache_id != u32::MAX
                    && let Some(v) = vm.t2_site_verdict.get_mut(cache_id as usize)
                {
                    *v = 0;
                }
                if vm.jit_stats_on {
                    vm.t2_call_stats[0] += 1;
                }
                return t2_finish(vm, depth);
            }
            Ok(false) => {
                // Count the wasted probe toward the site's settle verdict.
                if cache_id != u32::MAX {
                    let idx = cache_id as usize;
                    if vm.t2_site_verdict.len() <= idx {
                        vm.t2_site_verdict.resize(idx + 1, 0);
                    }
                    vm.t2_site_verdict[idx] = vm.t2_site_verdict[idx].saturating_add(1);
                }
            }
            Err(t) => {
                vm.t2_trap = Some(t);
                return T2_TRAP;
            }
        }
    }
    // Fallback: the interpreter's own arm (the full `do_call` cascade),
    // including the explicit-brace trailing-Hash-is-positional flag.
    if vm.jit_stats_on {
        vm.t2_call_stats[1] += 1;
    }
    // TEMPORARY census (`RUBYRS_T2_FALLBACK_STATS=1`): classify this
    // fallback edge (reason × name × shape × argc) and mark the
    // dispatch below as t2-originating for the slow-cascade cross-tab.
    if vm.t2_fb_stats.is_some() {
        let path = if fast {
            2 // the probe ran and declined
        } else if site_v >= T2_SITE_SETTLE {
            1 // settled-site skip (a chronic decliner — classify anyway)
        } else {
            0 // dispatch-boundary gate
        };
        vm.t2_fb_classify_call(name_id, argc, no_recv, cache_id, path);
        vm.t2_fb_from = true;
    }
    vm.trailing_hash_positional = true;
    let r = vm.do_call(name_id, argc, no_recv, cache_id);
    vm.trailing_hash_positional = false;
    if let Err(t) = r {
        vm.t2_trap = Some(t);
        return T2_TRAP;
    }
    t2_finish(vm, depth)
}

/// Per-call-op prologue shared by the t2_call family: advance `ip` past the
/// op (backtraces/resume key off it) and charge the fuel tick `step()` would
/// have charged — BEFORE any stack effect, matching the interpreter's
/// fuel-then-arm order.
#[inline]
fn t2_call_prologue(vm: &mut crate::vm::Vm, ip: i64) -> Result<(), ()> {
    vm.frames
        .last_mut()
        .expect("ICE: t2_call with empty frame stack")
        .ip = ip as usize + 1;
    if let Err(t) = vm.check_fuel() {
        vm.t2_trap = Some(t);
        return Err(());
    }
    Ok(())
}

/// `Op::Call(name, argc, cid)` — explicit receiver.
unsafe extern "C" fn t2_call(
    vm: *mut crate::vm::Vm,
    name: i64,
    argc: i64,
    cid: i64,
    ip: i64,
) -> i64 {
    let vm = unsafe { &mut *vm };
    if t2_call_prologue(vm, ip).is_err() {
        return T2_TRAP;
    }
    t2_call_impl(vm, SymId(name as u32), argc as usize, cid as u32, false)
}

/// `Op::CallNoRecv(name, argc, cid)` — implicit self.
unsafe extern "C" fn t2_call_norecv(
    vm: *mut crate::vm::Vm,
    name: i64,
    argc: i64,
    cid: i64,
    ip: i64,
) -> i64 {
    let vm = unsafe { &mut *vm };
    if t2_call_prologue(vm, ip).is_err() {
        return T2_TRAP;
    }
    t2_call_impl(vm, SymId(name as u32), argc as usize, cid as u32, true)
}

/// `Op::LoadLocalCall(slot, name, cid)` — the fused superinstruction: push
/// the local receiver (mirrors `Op::LoadLocal`), then the same zero-arg
/// explicit-recv dispatch. The pushed local lives on `vm.stack` (a GC root)
/// for the whole dispatch.
unsafe extern "C" fn t2_call_local(
    vm: *mut crate::vm::Vm,
    slot: i64,
    name: i64,
    cid: i64,
    ip: i64,
) -> i64 {
    let vm = unsafe { &mut *vm };
    if t2_call_prologue(vm, ip).is_err() {
        return T2_TRAP;
    }
    let f = vm.frames.last().expect("ICE: t2_call_local no frame");
    // Capture-routed read — mirrors the step arm's Op::LoadLocal.
    let v = crate::vm::Vm::frame_local_get(f, &vm.locals_arena, slot as usize);
    vm.stack.push(v);
    t2_call_impl(vm, SymId(name as u32), 0, cid as u32, false)
}

/// Lean `Op::Super(name, argc, cid)` serve (campaign P5a): the step
/// arm's body — drain the argc args, run
/// `Vm::super_call_with_lifecycle_noop` (the P4 super-site-cached
/// resolve + invoke, error shapes and the lifecycle-noop intercept
/// included) — behind the t2_call family's own boundary instead of
/// the generic `t2_op` (op decode + `Vm::step` match; the AM
/// census's Super 71/iter row). The frame push is CONTAINED exactly
/// as for the call family: `t2_call_prologue` stamps `ip` past the
/// op + charges the fuel tick `step()` would have charged, and
/// `t2_finish` drives any pushed callee frame to completion /
/// reports bail statuses — byte for byte the machinery `t2_op` used
/// for this op.
unsafe extern "C" fn t2_super(
    vm: *mut crate::vm::Vm,
    name: i64,
    argc: i64,
    cid: i64,
    ip: i64,
) -> i64 {
    let vm = unsafe { &mut *vm };
    if t2_call_prologue(vm, ip).is_err() {
        return T2_TRAP;
    }
    let depth = vm.frames.len();
    let split = vm.stack.len() - argc as usize;
    let args: Vec<Value> = vm.stack.drain(split..).collect();
    if let Err(t) = vm.super_call_with_lifecycle_noop(SymId(name as u32), args, cid as u32) {
        vm.t2_trap = Some(t);
        return T2_TRAP;
    }
    t2_finish(vm, depth)
}

/// Shared miss path for the framed const helpers: exactly one generic-
/// helper iteration (`t2_op`'s body with the op value rebuilt — no baked
/// pointer needed): stamp `ip` past the op, run the interpreter's own arm
/// (autoload / qualified-path walk / const_missing / NameError / cache
/// refill), drive any frames it pushed, report the exit status.
#[cold]
fn t2_const_miss(vm: &mut crate::vm::Vm, op: Op, ip: i64) -> i64 {
    let depth = vm.frames.len();
    let pidx = {
        let f = vm
            .frames
            .last_mut()
            .expect("ICE: t2_const with empty frame stack");
        f.ip = ip as usize + 1;
        f.proto_idx
    };
    if vm.t2_op_stats.is_some() {
        t2_census_note_op(vm, &op);
    }
    if let Err(t) = vm.step(op, pidx) {
        vm.t2_trap = Some(t);
        return T2_TRAP;
    }
    t2_finish(vm, depth)
}

/// `Op::LoadConst(sym)` in a FRAMED tier-2 body (ADR 0037 tail): the
/// interpreter's own flat inline-constant cache served without the generic
/// `step()` round-trip. Hit (generation-tagged, and the name is not a
/// registered `private_constant` — the interpreter arm runs that check
/// BEFORE its cache read, so it gates the fast path too) → clone-push +
/// CONTINUE; miss → the interpreter's full arm via `t2_const_miss`. Like
/// the specialized simple ops, the hit path charges no fuel tick and
/// stamps no `ip` (nothing after the push can fault).
unsafe extern "C" fn t2_const_flat(vm: *mut crate::vm::Vm, sym: i64, ip: i64) -> i64 {
    let vm = unsafe { &mut *vm };
    let name_id = SymId(sym as u32);
    if (vm.private_consts.is_empty() || !vm.private_consts.contains(&name_id))
        && let Some((v, g)) = vm.const_cache_flat.get(&name_id)
        && *g == vm.const_gen
    {
        let v = v.clone();
        vm.stack.push(v);
        return T2_CONTINUE;
    }
    t2_const_miss(vm, Op::LoadConst(name_id), ip)
}

/// `Op::LoadConstChain(idx)` in a FRAMED tier-2 body: the `(proto, chain
/// slot)` inline constant cache, same hit/miss split as `t2_const_flat`.
/// `pidx` is baked by the codegen (a framed body only ever runs against
/// its own frame, so it equals `frame.proto_idx` — the interpreter arm's
/// cache key).
unsafe extern "C" fn t2_const_chain(vm: *mut crate::vm::Vm, ci: i64, pidx: i64, ip: i64) -> i64 {
    let vm = unsafe { &mut *vm };
    let key = (pidx as u32, ci as u32);
    if let Some((v, g)) = vm.const_cache_chain.get(&key)
        && *g == vm.const_gen
    {
        let v = v.clone();
        vm.stack.push(v);
        return T2_CONTINUE;
    }
    t2_const_miss(vm, Op::LoadConstChain(ci as u32), ip)
}

/// `Op::CallBlock` / `Op::CallNoRecvBlock` — the block-passing call ops
/// (wave 5). Mirrors the step arms byte for byte: the explicit-brace
/// trailing-Hash-is-positional flag is set around `do_call_block` exactly
/// like `Op::CallBlock`'s arm (a `k: v`/`**h` + block call compiles to
/// `CallKwBlock`, which stays on the generic helper). `do_call_block`
/// itself front-loads the block-form IC fast path
/// (`try_invoke_explicit_recv_block_cached`, whose trailing `t2_enter`
/// runs a compiled callee natively), builds the BlockHandle through the
/// interpreter's own `CreateBlock` arm (already executed as a prior op of
/// this body — outer-chain flattening and all), and the callee's `yield`
/// serves the compiled block via the `do_yield` hook — the full
/// native→native block-passing chain.
#[inline]
fn t2_call_block_impl(
    vm: &mut crate::vm::Vm,
    name_id: SymId,
    argc: usize,
    cache_id: u32,
    no_recv: bool,
) -> i64 {
    let depth = vm.frames.len();
    // TEMPORARY census (`RUBYRS_T2_FALLBACK_STATS=1`): reason 18 =
    // every in-body block-form call (do_call_block's front-loaded
    // block IC may still serve it natively; reason 19 inside
    // `do_call_block` tags the subset that fell past that IC).
    // Stack here: [.., recv?, block, a1..aN].
    if vm.t2_fb_stats.is_some() {
        let shape = if no_recv {
            12
        } else {
            vm.stack
                .len()
                .checked_sub(argc + 2)
                .map_or(11, |i| crate::vm::Vm::t2_fb_shape(&vm.stack[i]))
        };
        vm.t2_fb_record(18, name_id, shape, argc);
        vm.t2_fb_from = true;
    }
    vm.trailing_hash_positional = true;
    let r = vm.do_call_block(name_id, argc, no_recv, cache_id);
    vm.trailing_hash_positional = false;
    if let Err(t) = r {
        vm.t2_trap = Some(t);
        return T2_TRAP;
    }
    t2_finish(vm, depth)
}

/// `Op::CallBlock(name, argc, cid)` — explicit receiver, literal block.
unsafe extern "C" fn t2_call_block(
    vm: *mut crate::vm::Vm,
    name: i64,
    argc: i64,
    cid: i64,
    ip: i64,
) -> i64 {
    let vm = unsafe { &mut *vm };
    if t2_call_prologue(vm, ip).is_err() {
        return T2_TRAP;
    }
    t2_call_block_impl(vm, SymId(name as u32), argc as usize, cid as u32, false)
}

/// `Op::CallNoRecvBlock(name, argc, cid)` — implicit self, literal block.
unsafe extern "C" fn t2_call_norecv_block(
    vm: *mut crate::vm::Vm,
    name: i64,
    argc: i64,
    cid: i64,
    ip: i64,
) -> i64 {
    let vm = unsafe { &mut *vm };
    if t2_call_prologue(vm, ip).is_err() {
        return T2_TRAP;
    }
    t2_call_block_impl(vm, SymId(name as u32), argc as usize, cid as u32, true)
}

/// `Op::Yield(n)` / `Op::ApplyYield` (wave 5): run the interpreter's own
/// yield executor (`Vm::do_yield`, the extracted `Op::Yield` arm — the
/// single source of truth for the lexical-owner walk, `YieldDepthGuard`,
/// and the break/fiber/non-local-return postlude) without the `step`
/// fetch/decode round-trip. The arm's `invoke_block` is followed by the
/// wave-5 `t2_enter_block` hook, so a compiled method yielding to a
/// compiled block runs the block body natively (native→native yield).
/// `argc` carries `Op::Yield`'s static count, or -1 for `Op::ApplyYield`
/// (the arm pops + expands the args Array itself). Status mapping matches
/// `t2_op`: a break consuming this very frame (yield case (a)) lands as
/// frames-below-entry → DONE with the method's return value placed;
/// pending signals (case (b) parked breaks, non-local returns, fiber
/// yields) → BAIL with `ip` at the resume point.
unsafe extern "C" fn t2_yield(vm: *mut crate::vm::Vm, argc: i64, ip: i64) -> i64 {
    let vm = unsafe { &mut *vm };
    let depth = vm.frames.len();
    {
        let f = vm.frames.last_mut().expect("ICE: t2_yield with empty frame stack");
        f.ip = ip as usize + 1;
    }
    // `step` charges the fuel tick before its arm; mirror it.
    if let Err(t) = vm.check_fuel() {
        vm.t2_trap = Some(t);
        return T2_TRAP;
    }
    let op = if argc < 0 { Op::ApplyYield } else { Op::Yield(argc as u8) };
    if let Err(t) = vm.do_yield(op) {
        vm.t2_trap = Some(t);
        return T2_TRAP;
    }
    t2_finish(vm, depth)
}

/// `Op::Return` core — the frame-pop shortcut (wave 2): mirrors `step()`'s
/// `Op::Return` arm without the fetch/decode/match round-trip, so a
/// native→native callee returns straight to its caller's native code. The
/// two cold shapes — a pending `is_ensure` handler (unreachable for an
/// admitted body: admission declines `PushEnsure`; kept for exactness) and a
/// class-body frame (tier-2 frames are method frames by construction) —
/// route through the interpreter's own arm via `step`. The hot path is the
/// arm's plain direct-pop, byte for byte: `$~` restore, `$!` restore, pop +
/// truncate-to-`base_sp` + push return (honoring `swap_return`), then the
/// `release_frame_locals` / `recycle_frame_aux` recycling discipline
/// (3397804a). `reg_ret`: `Some(v)` when the return value arrives in
/// registers (the wave-3 `t2_return_v` — the value was virtual, never
/// materialized); `None` pops it from the operand stack (wave-2 shape).
fn t2_return_impl(vm: &mut crate::vm::Vm, pidx: i64, ip: i64, reg_ret: Option<Value>) -> i64 {
    let depth = vm.frames.len();
    {
        let f = vm
            .frames
            .last_mut()
            .expect("ICE: t2_return with empty frame stack");
        f.ip = ip as usize + 1;
    }
    let top = vm.frames.last().expect("ICE: t2_return no frame");
    let cold = top.is_class_body
        || top
            .aux
            .as_ref()
            .is_some_and(|a| a.rescues.iter().any(|h| h.is_ensure));
    if cold {
        // Full interpreter semantics (ensure-walk / class-body return);
        // `step` charges its own fuel tick. A register-borne return value
        // is materialized first so the arm sees the interpreter's state.
        if let Some(v) = reg_ret {
            vm.stack.push(v);
        }
        if let Err(t) = vm.step(Op::Return, pidx as usize) {
            vm.t2_trap = Some(t);
            return T2_TRAP;
        }
        return t2_finish(vm, depth);
    }
    if let Err(t) = vm.check_fuel() {
        vm.t2_trap = Some(t);
        return T2_TRAP;
    }
    let f = vm.frames.pop().expect("ICE: t2_return no frame");
    if f.dm_share {
        vm.dm_share_depth = vm.dm_share_depth.saturating_sub(1);
    }
    // Frame-local `$~` restore (see the step arm's comment).
    #[cfg(feature = "regex")]
    if let Some(saved) = f.saved_last_match {
        vm.last_match = saved.map(|b| *b);
    }
    // `$!` restore to the outermost still-open begin's snapshot (dynamically
    // scoped errinfo — see the step arm). Always empty for admitted bodies
    // (`EnterBegin` declines); mirrored for exactness.
    if let Some(saved) = f
        .aux
        .as_ref()
        .and_then(|a| a.begin_rescue_depths.first())
        .map(|b| b.saved_dollar_bang.clone())
    {
        vm.globals.insert(vm.sym_bang, saved);
    }
    let ret = match reg_ret {
        Some(v) => v,
        None => vm.stack.pop().unwrap_or(Value::Nil),
    };
    vm.stack.truncate(f.base_sp);
    if let Some(replacement) = f.swap_return {
        vm.stack.push(replacement);
    } else {
        vm.stack.push(ret);
    }
    vm.release_frame_locals(f.locals);
    vm.recycle_frame_aux(f.aux);
    T2_DONE
}

unsafe extern "C" fn t2_return(vm: *mut crate::vm::Vm, pidx: i64, ip: i64) -> i64 {
    let vm = unsafe { &mut *vm };
    t2_return_impl(vm, pidx, ip, None)
}

/// `Op::Return` with the return value in registers (wave 3): the value was
/// a virtual (trivially-tagged) stack top — ownership transfers here, the
/// operand-stack round trip is skipped entirely.
unsafe extern "C" fn t2_return_v(
    vm: *mut crate::vm::Vm,
    w0: i64,
    w1: i64,
    pidx: i64,
    ip: i64,
) -> i64 {
    let vm = unsafe { &mut *vm };
    let v = unsafe { value_from_words([w0, w1]) };
    t2_return_impl(vm, pidx, ip, Some(v))
}

/// `Op::JumpIfFalse` condition: pop + truthiness. Returns 1 when truthy
/// (fall through), 0 when falsy (take the jump). Mirrors the step arm.
unsafe extern "C" fn t2_pop_truthy(vm: *mut crate::vm::Vm) -> i64 {
    let vm = unsafe { &mut *vm };
    let v = vm.stack.pop().expect("ICE: JumpIfFalse stack underflow");
    v.is_truthy() as i64
}

/// `Op::JumpIfArgGiven`: 1 when positional `slot` was caller-supplied.
unsafe extern "C" fn t2_arg_given(vm: *mut crate::vm::Vm, slot: i64) -> i64 {
    let vm = unsafe { &mut *vm };
    let f = vm.frames.last().expect("ICE: JumpIfArgGiven no frame");
    ((slot as u16) < f.n_given_positional) as i64
}

/// `Op::JumpIfKwArgGiven`: 1 when kwarg index `kw_idx` was caller-supplied.
unsafe extern "C" fn t2_kwarg_given(vm: *mut crate::vm::Vm, kw_idx: i64) -> i64 {
    let vm = unsafe { &mut *vm };
    let f = vm.frames.last().expect("ICE: JumpIfKwArgGiven no frame");
    let kw_idx = kw_idx as u16;
    (kw_idx < 64 && (f.kw_given_mask & (1u64 << kw_idx)) != 0) as i64
}

unsafe extern "C" fn t2_push_int(vm: *mut crate::vm::Vm, n: i64) {
    let vm = unsafe { &mut *vm };
    vm.stack.push(Value::Int(n));
}

unsafe extern "C" fn t2_push_nil(vm: *mut crate::vm::Vm) {
    let vm = unsafe { &mut *vm };
    vm.stack.push(Value::Nil);
}

unsafe extern "C" fn t2_push_bool(vm: *mut crate::vm::Vm, b: i64) {
    let vm = unsafe { &mut *vm };
    vm.stack.push(Value::Bool(b != 0));
}

unsafe extern "C" fn t2_push_sym(vm: *mut crate::vm::Vm, id: i64) {
    let vm = unsafe { &mut *vm };
    vm.stack.push(Value::Sym(SymId(id as u32)));
}

unsafe extern "C" fn t2_load_self(vm: *mut crate::vm::Vm) {
    let vm = unsafe { &mut *vm };
    let v = vm.frames.last().expect("ICE: LoadSelf no frame").self_val.clone();
    vm.stack.push(v);
}

unsafe extern "C" fn t2_load_local(vm: *mut crate::vm::Vm, slot: i64) {
    let vm = unsafe { &mut *vm };
    let f = vm.frames.last().expect("ICE: LoadLocal no frame");
    // Capture-routed read — mirrors the step arm's Op::LoadLocal.
    let v = crate::vm::Vm::frame_local_get(f, &vm.locals_arena, slot as usize);
    vm.stack.push(v);
}

unsafe extern "C" fn t2_store_local(vm: *mut crate::vm::Vm, slot: i64) {
    let vm = unsafe { &mut *vm };
    let v = vm.stack.pop().expect("ICE: StoreLocal stack underflow");
    let slot = slot as usize;
    let frame = vm.frames.last().expect("ICE: StoreLocal no frame");
    match &frame.locals {
        crate::vm::Locals::Stack(base) => {
            let idx = *base as usize + slot;
            vm.locals_arena[idx] = v;
        }
        crate::vm::Locals::Shared(rc) => {
            // Capture-routed write — mirrors the step arm.
            if let Some(cell) = frame.outer_cell_for(slot) {
                crate::vm::cell_store(cell, slot, v);
            } else {
                rc.borrow_mut()[slot] = v;
            }
        }
    }
}

unsafe extern "C" fn t2_load_ivar(vm: *mut crate::vm::Vm, name_id: i64, _cid: i64) {
    let vm = unsafe { &mut *vm };
    let name_id = SymId(name_id as u32);
    let self_val = vm.frames.last().expect("ICE: LoadIvar no frame").self_val.clone();
    let v = match &self_val {
        Value::Object(id) => {
            let inst = vm.heap.instance(*id);
            match inst.class.ivar_slot_lookup_fast(name_id) {
                Some(slot) => inst.ivars.read_slot_raw(slot),
                None => Value::Nil,
            }
        }
        Value::Class(c) => c.ivars.borrow().get(&name_id).cloned().unwrap_or(Value::Nil),
        Value::Hash(id) => vm.heap.hash_ivar_get(*id, name_id).unwrap_or(Value::Nil),
        Value::Array(id) => vm.heap.array_ivar_get(*id, name_id).unwrap_or(Value::Nil),
        Value::Str(s) => {
            let key = std::rc::Rc::as_ptr(s) as usize;
            vm.str_ivars
                .get(&key)
                .and_then(|(_, m)| m.get(&name_id).cloned())
                .unwrap_or(Value::Nil)
        }
        _ => Value::Nil,
    };
    vm.stack.push(v);
}

/// Lean `Op::LoadCvar` serve (campaign P4): the interpreter arm verbatim
/// (`Vm::cvar_load` — surrounding-class resolve + per-site owner cache +
/// value read). Never traps, never pushes frames, never allocates on the
/// GC heap → no ip stamp / status needed (same contract as t2_load_ivar).
unsafe extern "C" fn t2_load_cvar(vm: *mut crate::vm::Vm, name_id: i64, cid: i64) {
    let vm = unsafe { &mut *vm };
    let v = vm.cvar_load(SymId(name_id as u32), cid as u32);
    vm.stack.push(v);
}

/// Lean `Op::StoreCvar` serve: pops the stored value from the REAL
/// operand stack (the codegen flushes first) and runs `Vm::cvar_store`.
/// Same no-trap/no-frame/no-alloc contract as the load above.
unsafe extern "C" fn t2_store_cvar(vm: *mut crate::vm::Vm, name_id: i64, cid: i64) {
    let vm = unsafe { &mut *vm };
    let v = vm.stack.pop().expect("ICE: StoreCvar stack underflow");
    vm.cvar_store(SymId(name_id as u32), cid as u32, v);
}

unsafe extern "C" fn t2_dup(vm: *mut crate::vm::Vm) {
    let vm = unsafe { &mut *vm };
    let v = vm.stack.last().expect("ICE: Dup stack underflow").clone();
    vm.stack.push(v);
}

unsafe extern "C" fn t2_pop(vm: *mut crate::vm::Vm) {
    let vm = unsafe { &mut *vm };
    vm.stack.pop();
}

unsafe extern "C" fn t2_swap(vm: *mut crate::vm::Vm) {
    let vm = unsafe { &mut *vm };
    let n = vm.stack.len();
    vm.stack.swap(n - 1, n - 2);
}

// ---------------------------------------------------------------------------
// Admission + compilation
// ---------------------------------------------------------------------------

#[inline]
fn jump_target(i: usize, off: i32) -> usize {
    (i as i64 + 1 + off as i64) as usize
}

/// Admission: decline only the ops that install/consume rescue-or-ensure
/// handlers on THIS frame (an unwind could then redirect `frame.ip` into the
/// body and expect the INTERPRETER to resume there while the native code is
/// mid-flight) and the non-local-exit ops the master loop must own. Every
/// other op runs with full interpreter semantics via the helpers.
pub(crate) fn t2_admit(proto: &Proto) -> Result<(), String> {
    if proto.code.is_empty() {
        return Err("empty body".into());
    }
    let n = proto.code.len();
    for (i, op) in proto.code.iter().enumerate() {
        match op {
            Op::PushRescue(..)
            | Op::PushRescueSplatLocal(..)
            | Op::PopRescue
            | Op::EnterBegin
            | Op::ExitBegin
            | Op::TruncateRescuesToBeginBaseline
            | Op::PushEnsure(..)
            | Op::PopEnsure
            | Op::Raise
            | Op::EndEnsure
            | Op::ReturnMethod
            | Op::Break => return Err(format!("op {:?} at {}", op, i)),
            // Structural sanity for the native control flow: every branch
            // target must land on a real op.
            // Guard on the arm (clippy collapsible_match) — an
            // in-range target falls through to `_`, matching the
            // conditional-jump arm's shape below.
            Op::Jump(off) | Op::BreakLoop(off) | Op::NextLoop(off)
                if jump_target(i, *off) >= n =>
            {
                return Err(format!("jump target out of range at {}", i));
            }
            Op::JumpIfFalse(off) | Op::JumpIfArgGiven(_, off) | Op::JumpIfKwArgGiven(_, off)
                if jump_target(i, *off) >= n || i + 1 >= n =>
            {
                return Err(format!("cond target out of range at {}", i));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Frame-lite admission caps: bodies are the measured hot LEAF population
/// (getters / predicates / small setters); big bodies would pay entry spill
/// cost and bloat the native frame for no per-call win.
const LITE_MAX_OPS: usize = 48;
const LITE_MAX_LOCALS: u16 = 12;
const LITE_MAX_ARGC: u16 = 4;

/// FRAMED-tier cap for routing `Op::Call`/`Op::CallNoRecv` to the IC-fast
/// `t2_call` family instead of the generic helper. Was 2 through wave 5 (a
/// wave-2 conservatism, not an ABI limit — the helpers and every serve in
/// the matrix take `argc` dynamically); the 2026-07 fallback census found
/// ~15-20K calls/walk at argc 3-4 (`check_tokens`, `Class.new(a,b,c,d)`,
/// `__send__(:m, a, b)`) stuck on the `step()` round-trip for no reason.
const T2_CALL_MAX_ARGC: u8 = 8;

/// FRAME-LITE admission (wave 4, conservative leaf tier): the body may run
/// with NO interpreter frame at all, so every admitted op must either be
/// fully servable by the frameless inline lowering / the `t2_lite_*` helper
/// family, or be safely abandonable to `t2_lite_materialize` + BAIL before
/// any of its effects. Declines:
///
/// - non-method protos (block bodies bind `Locals::Shared`) and
///   `creates_block` bodies (a capture needs a real cell),
/// - any non-plain parameter shape (optionals/rest/post/kw/kw-rest/`&blk` —
///   their binders and `JumpIfArgGiven`/`JumpIfKwArgGiven` prologues read
///   the frame's arity model),
/// - every op outside the enumerated leaf set (anything that could read
///   `$~`, cvars, globals, the block arg, or run arbitrary interpreter
///   arms).
///
/// LITE t2_call (wave-4 follow-on): the plain fixed-argc call ops
/// (`Call`/`CallNoRecv`/`LoadLocalCall`, argc ≤ `LITE_MAX_ARGC`) and the
/// IC-cached `LoadConstChain` ARE admitted — served by the `t2_lite_call_*`
/// helper family, whose every edge either completes frameless or
/// materializes the frame (with outward-in cascading through nested lite
/// activations) before the interpreter takes over. Block-passing and
/// kw-bearing call forms still decline the body.
///
/// Returns the plain fixed argc the entry bakes.
pub(crate) fn t2_admit_lite(proto: &Proto, ctx: &T2Ctx) -> Result<u16, String> {
    if proto.creates_block {
        return Err("creates_block".into());
    }
    if proto.block_body_local_start != u16::MAX {
        return Err("block proto".into());
    }
    if proto.n_optional_params != 0
        || proto.rest_param.is_some()
        || !proto.kw_param_defaults.is_empty()
        || proto.kw_rest_param.is_some()
        || proto.block_param.is_some()
        || proto.n_required_post != 0
    {
        return Err("non-plain params".into());
    }
    let argc = proto.n_required_positional;
    if argc > LITE_MAX_ARGC {
        return Err("argc cap".into());
    }
    if proto.n_locals > LITE_MAX_LOCALS || proto.n_locals < argc {
        return Err("locals cap".into());
    }
    let n = proto.code.len();
    if n == 0 || n > LITE_MAX_OPS {
        return Err("size cap".into());
    }
    let nl = proto.n_locals;
    for (i, op) in proto.code.iter().enumerate() {
        match op {
            Op::LoadConstInt(_)
            | Op::LoadConstFloat(_)
            | Op::LoadNil
            | Op::LoadTrue
            | Op::LoadFalse
            | Op::LoadSymbol(_)
            | Op::LoadSelf
            | Op::LoadIvar(..)
            | Op::StoreIvar(..)
            | Op::Dup
            | Op::Pop
            | Op::Swap
            | Op::Return => {}
            Op::LoadLocal(s) | Op::StoreLocal(s) | Op::IncLocal(s) | Op::IncLocalNoPush(s)
                if *s < nl => {}
            Op::BinOp(k) | Op::BinOpInt(k, _) if binop_inlineable(*k) => {}
            Op::BinOpLocalLocal(k, a, b) if binop_inlineable(*k) && *a < nl && *b < nl => {}
            Op::CaseEqLit(lit, _) if case_lit_kind(lit).is_some() => {}
            Op::Jump(off) => {
                if jump_target(i, *off) >= n {
                    return Err(format!("jump target out of range at {}", i));
                }
            }
            Op::JumpIfFalse(off) => {
                if jump_target(i, *off) >= n || i + 1 >= n {
                    return Err(format!("cond target out of range at {}", i));
                }
            }
            // The zero-arg `x.nil?` fusion (same gates as the inline arm).
            Op::Call(name, 0, _) if name.0 == ctx.sym_nil_q => {}
            // LITE t2_call: plain fixed-argc call ops + IC-cached consts +
            // the (infallible, GC-free: `Value::Str` is `Rc`-backed) fresh
            // string-literal push.
            Op::Call(_, a, _) | Op::CallNoRecv(_, a, _) if (*a as u16) <= LITE_MAX_ARGC => {}
            Op::LoadLocalCall(s, _, _) if *s < nl => {}
            Op::LoadConstChain(_) | Op::LoadConst(_) | Op::LoadConstStr(_) => {}
            other => return Err(format!("op {:?} at {}", other, i)),
        }
    }
    Ok(argc)
}

/// LITE-BLOCK admission (ADR 0037 block-frame residue): a BLOCK body may run
/// with no block frame at all. On top of the wave-4 envelope, the block
/// wrinkles:
///
/// - plain call interface only: `n_params ≤ 1`, no rest / kw-rest / named-kw
///   / `&param`, no optionals, and no gap slots between the params and the
///   body-local region (`block_body_local_start == param_start + n_params`)
///   so the entry's Nil-init mirrors the interpreter binder exactly;
/// - own-region ops get the wave-3/4 lowering against the native spill;
///   captured-outer slots (`< param_start`) are admitted for plain
///   `LoadLocal`/`StoreLocal` (served by the `t2_lite_blk_outer_*`
///   cell-routing helpers) but NOT for the slot-fused forms
///   (`IncLocal`/`BinOpLocalLocal`/`LoadLocalCall`) whose lowerings read
///   the spill directly;
/// - `next` is the block's `Op::Return` (a frameless return);
///   `break`/`return`-from-block (`Op::Break`/`Op::ReturnMethod`) already
///   decline `t2_admit`, so no lite body can contain them — those shapes
///   stay interpreted end-to-end;
/// - `creates_block` declines (an inner capture needs a real cell), which
///   also keeps every own-region slot un-escaped while it lives in the
///   spill;
/// - the rest-only `|*a|` shape (ADR 0037 tail; the walk's hottest binder
///   shape) IS admitted: by `compile_block`'s slot layout its rest slot is
///   exactly `ps` (the only param-interface slot) with the body region at
///   `ps + 1`, so the entry compiles as a plain 1-param binder whose one
///   arg is the rest Array the serve site pre-allocates — binding, spill
///   classification and the materialize path are the 1-param entry's
///   verbatim (`push_lite_block_frame` writes the array at `ps` ==
///   `rest_slot`, exactly where `invoke_block1`'s framed rest arm binds
///   it). Rest WITH fixed params (`|a, *b|`) declines — its overflow
///   collection is genuinely variadic.
///
/// Returns the baked `(param_start, n_params_bound, is_rest)` —
/// `n_params_bound` counts the operand-stack slots the entry pops (1 for
/// the rest shape: the pre-built Array).
pub(crate) fn t2_admit_lite_block(proto: &Proto, ctx: &T2Ctx) -> Result<(u16, u16, bool), String> {
    let Some((ps, np, has_rest, has_kwrest)) = proto.block_shape else {
        return Err("no block shape".into());
    };
    if proto.creates_block {
        return Err("creates_block".into());
    }
    if has_kwrest || np > 2 {
        return Err("non-plain block params".into());
    }
    if has_rest && np != 0 {
        return Err("rest with fixed params".into());
    }
    if !proto.block_kw_params.is_empty()
        || proto.block_param_slot.is_some()
        || proto.n_optional_params != 0
    {
        return Err("kw/block/optional params".into());
    }
    // The rest-only entry binds ONE stack slot: the pre-allocated Array.
    let np = if has_rest { 1 } else { np };
    if proto.block_body_local_start != ps + np {
        return Err("param/body gap".into());
    }
    if proto.n_locals < ps + np {
        return Err("slot layout".into());
    }
    // Own region within the wave-4 cap; the whole spill (which is sized
    // n_locals and includes the never-touched outer prefix) within the
    // meta encoding's 8-bit n_locals field.
    if proto.n_locals - ps > LITE_MAX_LOCALS || proto.n_locals > 200 {
        return Err("locals cap".into());
    }
    let n = proto.code.len();
    if n == 0 || n > LITE_MAX_OPS {
        return Err("size cap".into());
    }
    let nl = proto.n_locals;
    for (i, op) in proto.code.iter().enumerate() {
        match op {
            Op::LoadConstInt(_)
            | Op::LoadConstFloat(_)
            | Op::LoadNil
            | Op::LoadTrue
            | Op::LoadFalse
            | Op::LoadSymbol(_)
            | Op::LoadSelf
            | Op::LoadIvar(..)
            | Op::StoreIvar(..)
            | Op::Dup
            | Op::Pop
            | Op::Swap
            | Op::Return => {}
            // Plain local reads/writes: own-region slots lower against the
            // spill; outer slots route through the cell helpers.
            Op::LoadLocal(s) | Op::StoreLocal(s) if *s < nl => {}
            // Slot-fused forms read the spill directly — own region only.
            Op::IncLocal(s) | Op::IncLocalNoPush(s) if *s >= ps && *s < nl => {}
            Op::BinOp(k) | Op::BinOpInt(k, _) if binop_inlineable(*k) => {}
            // Fused two-slot arithmetic: outer operands are served by the
            // effect-free register read (`t2_lite_blk_outer_read`), so any
            // in-range slot pair admits.
            Op::BinOpLocalLocal(k, a, b)
                if binop_inlineable(*k) && *a < nl && *b < nl => {}
            Op::CaseEqLit(lit, _) if case_lit_kind(lit).is_some() => {}
            Op::Jump(off) => {
                if jump_target(i, *off) >= n {
                    return Err(format!("jump target out of range at {}", i));
                }
            }
            Op::JumpIfFalse(off) => {
                if jump_target(i, *off) >= n || i + 1 >= n {
                    return Err(format!("cond target out of range at {}", i));
                }
            }
            Op::Call(name, 0, _) if name.0 == ctx.sym_nil_q => {}
            Op::Call(_, a, _) | Op::CallNoRecv(_, a, _) if (*a as u16) <= LITE_MAX_ARGC => {}
            Op::LoadLocalCall(s, _, _) if *s >= ps && *s < nl => {}
            Op::LoadConstChain(_) | Op::LoadConst(_) | Op::LoadConstStr(_) => {}
            other => return Err(format!("op {:?} at {}", other, i)),
        }
    }
    Ok((ps, np, has_rest))
}

/// Ops that end a straight-line segment: their `step` arm may retarget
/// `frame.ip` (branches) or pop the frame (`Return`). Everything else in an
/// admitted body advances `ip` by exactly 1.
#[inline]
fn is_sync_op(op: &Op) -> bool {
    matches!(
        op,
        Op::Jump(_)
            | Op::JumpIfFalse(_)
            | Op::JumpIfArgGiven(..)
            | Op::JumpIfKwArgGiven(..)
            | Op::BreakLoop(_)
            | Op::NextLoop(_)
            | Op::Return
    )
}

// ---------------------------------------------------------------------------
// Wave-3 codegen: the virtual operand stack + local read cache + inline
// lowering of the hot ops (ADR 0037 wave 3).
// ---------------------------------------------------------------------------

/// A virtual operand-stack entry: a value that logically sits on top of the
/// operand stack but currently lives only in SSA registers, as the raw
/// 16-byte words of a `Value`. INVARIANT: the tag is always in
/// `trivial_mask` (guarded at creation), so duplication is a register copy,
/// discarding is free, and materialization is two plain stores.
#[derive(Clone, Copy)]
struct VVal {
    w0: ClValue,
    w1: ClValue,
    /// Compile-time-known tag (literals, arithmetic results).
    tag: Option<u8>,
    /// For Bool values born from a native compare: the raw 0/1 (i64), so a
    /// following JumpIfFalse fuses to a single brif.
    bit: Option<ClValue>,
    /// Compile-time-known truthiness (literals).
    truthy: Option<bool>,
}

impl VVal {
    fn raw(w0: ClValue, w1: ClValue) -> Self {
        VVal { w0, w1, tag: None, bit: None, truthy: None }
    }
}

/// Where a 2-operand op's input came from: an already-popped virtual entry,
/// or a real stack slot that was PEEKED (loads only) and is consumed by the
/// caller's `len -= n_real` once every guard has passed.
struct Operand {
    w0: ClValue,
    w1: ClValue,
    tag: Option<u8>,
}

struct HelperRefs {
    op: cranelift_codegen::ir::FuncRef,
    resume: cranelift_codegen::ir::FuncRef,
    entry_info: cranelift_codegen::ir::FuncRef,
    stack_reserve: cranelift_codegen::ir::FuncRef,
    poll: cranelift_codegen::ir::FuncRef,
    ivar_get: cranelift_codegen::ir::FuncRef,
    ivar_set_v: cranelift_codegen::ir::FuncRef,
    case_eq_v: cranelift_codegen::ir::FuncRef,
    case_eq_s: cranelift_codegen::ir::FuncRef,
    return_v: cranelift_codegen::ir::FuncRef,
    pop_truthy: cranelift_codegen::ir::FuncRef,
    arg_given: cranelift_codegen::ir::FuncRef,
    kwarg_given: cranelift_codegen::ir::FuncRef,
    push_int: cranelift_codegen::ir::FuncRef,
    push_nil: cranelift_codegen::ir::FuncRef,
    push_bool: cranelift_codegen::ir::FuncRef,
    push_sym: cranelift_codegen::ir::FuncRef,
    load_self: cranelift_codegen::ir::FuncRef,
    load_local: cranelift_codegen::ir::FuncRef,
    store_local: cranelift_codegen::ir::FuncRef,
    load_ivar: cranelift_codegen::ir::FuncRef,
    load_cvar: cranelift_codegen::ir::FuncRef,
    store_cvar: cranelift_codegen::ir::FuncRef,
    // Campaign P5a lean serves: stack-value StoreIvar + Op::Super.
    store_ivar: cranelift_codegen::ir::FuncRef,
    super_: cranelift_codegen::ir::FuncRef,
    // Campaign P6b lean serve: Op::InterpToS.
    interp_to_s: cranelift_codegen::ir::FuncRef,
    dup: cranelift_codegen::ir::FuncRef,
    pop: cranelift_codegen::ir::FuncRef,
    swap: cranelift_codegen::ir::FuncRef,
    call: cranelift_codegen::ir::FuncRef,
    call_norecv: cranelift_codegen::ir::FuncRef,
    call_local: cranelift_codegen::ir::FuncRef,
    ret: cranelift_codegen::ir::FuncRef,
    // Wave-5 block family: block-passing calls and yield keep their
    // dedicated helpers (do_call_block / do_yield front-loads) instead of
    // the generic per-op path.
    call_block: cranelift_codegen::ir::FuncRef,
    call_norecv_block: cranelift_codegen::ir::FuncRef,
    yield_: cranelift_codegen::ir::FuncRef,
    // Wave-4 frame-lite family.
    lite_mat: cranelift_codegen::ir::FuncRef,
    // Lite-block family (ADR 0037 block-frame residue).
    lite_mat_blk: cranelift_codegen::ir::FuncRef,
    blk_outer_get: cranelift_codegen::ir::FuncRef,
    blk_outer_read: cranelift_codegen::ir::FuncRef,
    blk_outer_set: cranelift_codegen::ir::FuncRef,
    lite_ret_v: cranelift_codegen::ir::FuncRef,
    lite_ret_s: cranelift_codegen::ir::FuncRef,
    lite_ivar_get: cranelift_codegen::ir::FuncRef,
    lite_ivar_set: cranelift_codegen::ir::FuncRef,
    // LITE t2_call family (wave-4 follow-on).
    lite_call_ex: cranelift_codegen::ir::FuncRef,
    lite_call_ns: cranelift_codegen::ir::FuncRef,
    lite_call_local: cranelift_codegen::ir::FuncRef,
    lite_const: cranelift_codegen::ir::FuncRef,
    // Const tail (ADR 0037): the lite flat-const read, the framed IC-hit
    // const reads, and the tier-shared string-literal push.
    lite_const_flat: cranelift_codegen::ir::FuncRef,
    const_flat: cranelift_codegen::ir::FuncRef,
    const_chain: cranelift_codegen::ir::FuncRef,
    push_const_str: cranelift_codegen::ir::FuncRef,
}

/// Snapshot of the compile-time machine state; slow-edge blocks materialize
/// the state AS OF the failing op from one of these.
#[derive(Clone)]
struct CgSnap {
    vst: Vec<VVal>,
    cache: Vec<Option<(ClValue, ClValue)>>,
    sptr: Option<ClValue>,
    slen: Option<ClValue>,
    scap: Option<ClValue>,
    aptr: Option<ClValue>,
}

/// How a straight-line segment's slow edge rejoins native control flow after
/// `t2_resume` has interpreted the remaining ops (INCLUDING the segment's
/// ending branch — its `step` arm retargets `frame.ip`, which `t2_resume`
/// reports back in the return value's high 32 bits).
#[derive(Clone, Copy)]
enum SyncKind {
    /// Resume stops at a leader (exclusive); native jumps there.
    Leader(usize),
    /// Resume ran THROUGH an unconditional branch; native jumps to its target.
    Uncond(usize),
    /// Resume ran THROUGH a conditional branch; dispatch on the landing ip:
    /// (taken target, fallthrough).
    Cond(usize, usize),
    /// Resume ran through `Return` — a CONTINUE status is impossible.
    Return,
}

/// The wave-3 codegen context. Compile-time state (the virtual stack, the
/// per-slot local read cache, the cached stack/arena registers) is reset at
/// every leader — control-flow merges therefore always see canonical state.
/// Within a straight-line run the fast path is a dominance CHAIN (guard
/// conts have a single predecessor; slow edges leave the segment), so SSA
/// values cached earlier in the run stay valid across helper calls.
struct Cg<'a> {
    ptr_ty: Type,
    vm: ClValue,
    tags: &'a T2Tags,
    lay: Option<VecLayout>,
    t2ctx: &'a T2Ctx,
    h: &'a HelperRefs,
    pidx: usize,
    code: &'a [Op],
    leader: &'a [bool],
    blocks: Vec<Option<Block>>,
    exit: Block,
    inline_on: bool,
    cacheable: bool,
    nocall: bool,
    /// Wave-4 FRAME-LITE emission mode: there is NO interpreter frame.
    /// Locals live in `lite_slot` (a native spill slot, write-through — the
    /// canonical local store while frameless); every slow edge materializes
    /// the frame (`fill_resume`'s lite branch) and BAILs instead of
    /// resuming; `Return` goes through the `t2_lite_return_*` helpers.
    lite: bool,
    /// Base address of the native locals spill slot (lite mode only).
    lite_slot: Option<ClValue>,
    /// Caller-context slot for the LITE t2_call helpers
    /// (`[slot_addr, trunc, n_pop, self_w0, self_w1]`), filled once at
    /// entry when the body contains admitted call/const ops.
    lite_ctx: Option<ClValue>,
    /// `entry stack len - n_pop` — the stack index of the recv slot
    /// (explicit) / the callee's would-be `base_sp` (lite mode only).
    lite_trunc: Option<ClValue>,
    /// The runtime `n_pop` entry parameter (lite mode only).
    lite_n_pop: Option<ClValue>,
    /// Baked plain-fixed argc (lite mode only).
    lite_argc: u16,
    /// LITE-BLOCK mode: this lite body is a BLOCK proto — the entry's 4th
    /// param is the BlockHandle id (not n_pop), outer slots
    /// (`< lite_ps`) route through the `t2_lite_blk_outer_*` helpers, and
    /// the bail edge materializes a BLOCK frame.
    lite_blk: bool,
    /// The block's own-region start (`param_start`; 0 in method mode).
    lite_ps: u16,
    /// The runtime block_id entry parameter (lite-block mode only).
    lite_blkid: Option<ClValue>,
    /// Per-backward-target poll blocks (shared: entry state is canonical),
    /// created on demand and filled after the main emission loop.
    poll_blocks: crate::intern::FxHashMap<usize, Block>,
    // --- compile-time machine state (reset at leaders) ---
    vst: Vec<VVal>,
    cache: Vec<Option<(ClValue, ClValue)>>,
    sptr: Option<ClValue>,
    slen: Option<ClValue>,
    scap: Option<ClValue>,
    aptr: Option<ClValue>,
    // --- entry-block registers (dominate everything; survive leaders) ---
    ebase16: Option<ClValue>,
    self_w0: Option<ClValue>,
    self_w1: Option<ClValue>,
    scratch: Option<ClValue>,
    // --- baked Vm field offsets ---
    off_stack: i32,
    off_arena: i32,
    off_signals: i32,
    off_poll_flags: i32,
    off_reopen: i32,
}

#[inline]
fn fl() -> MemFlagsData {
    MemFlagsData::new()
}

impl<'a> Cg<'a> {
    fn reset_block_state(&mut self) {
        self.vst.clear();
        for c in self.cache.iter_mut() {
            *c = None;
        }
        self.invalidate_mem();
    }

    fn snapshot(&self) -> CgSnap {
        CgSnap {
            vst: self.vst.clone(),
            cache: self.cache.clone(),
            sptr: self.sptr,
            slen: self.slen,
            scap: self.scap,
            aptr: self.aptr,
        }
    }

    fn restore(&mut self, s: CgSnap) {
        self.vst = s.vst;
        self.cache = s.cache;
        self.sptr = s.sptr;
        self.slen = s.slen;
        self.scap = s.scap;
        self.aptr = s.aptr;
    }

    /// Invalidate the cached stack/arena registers after any helper that may
    /// push/pop/grow the operand stack or grow the locals arena.
    fn invalidate_mem(&mut self) {
        self.sptr = None;
        self.slen = None;
        self.scap = None;
        self.aptr = None;
    }

    fn clear_cache(&mut self) {
        for c in self.cache.iter_mut() {
            *c = None;
        }
    }

    fn stack_ptr(&mut self, fb: &mut FunctionBuilder) -> ClValue {
        if let Some(p) = self.sptr {
            return p;
        }
        let lay = self.lay.expect("stack_ptr without probed layout");
        let p = fb.ins().load(self.ptr_ty, fl(), self.vm, self.off_stack + lay.ptr_off);
        self.sptr = Some(p);
        p
    }

    fn stack_len(&mut self, fb: &mut FunctionBuilder) -> ClValue {
        if let Some(l) = self.slen {
            return l;
        }
        let lay = self.lay.expect("stack_len without probed layout");
        let l = fb.ins().load(types::I64, fl(), self.vm, self.off_stack + lay.len_off);
        self.slen = Some(l);
        l
    }

    fn stack_cap(&mut self, fb: &mut FunctionBuilder) -> ClValue {
        if let Some(c) = self.scap {
            return c;
        }
        let lay = self.lay.expect("stack_cap without probed layout");
        let c = fb.ins().load(types::I64, fl(), self.vm, self.off_stack + lay.cap_off);
        self.scap = Some(c);
        c
    }

    fn set_stack_len(&mut self, fb: &mut FunctionBuilder, new_len: ClValue) {
        let lay = self.lay.expect("set_stack_len without probed layout");
        fb.ins().store(fl(), new_len, self.vm, self.off_stack + lay.len_off);
        self.slen = Some(new_len);
    }

    fn arena_ptr(&mut self, fb: &mut FunctionBuilder) -> ClValue {
        if let Some(p) = self.aptr {
            return p;
        }
        let lay = self.lay.expect("arena_ptr without probed layout");
        let p = fb.ins().load(self.ptr_ty, fl(), self.vm, self.off_arena + lay.ptr_off);
        self.aptr = Some(p);
        p
    }

    /// Base address of the frame's local slot `s`: the arena slot
    /// (`arena_ptr + (frame_base + s) * 16`) for framed bodies, the native
    /// spill slot for frame-lite bodies. Returned as (addr, imm_off).
    fn local_addr(&mut self, fb: &mut FunctionBuilder, s: u16) -> (ClValue, i32) {
        if self.lite {
            let base = self.lite_slot.expect("lite locals slot");
            return (base, (s as i32) * 16);
        }
        let ap = self.arena_ptr(fb);
        let base16 = self.ebase16.expect("local_addr without cacheable entry");
        let addr = fb.ins().iadd(ap, base16);
        (addr, (s as i32) * 16)
    }

    /// Address of the operand-stack slot `depth` entries below the top.
    fn stack_slot_addr(&mut self, fb: &mut FunctionBuilder, depth: i64) -> ClValue {
        let sp = self.stack_ptr(fb);
        let len = self.stack_len(fb);
        let idx = fb.ins().iadd_imm(len, -1 - depth);
        let off = fb.ins().ishl_imm(idx, 4);
        fb.ins().iadd(sp, off)
    }

    /// Materialize the whole virtual stack onto the real operand stack —
    /// the boundary discipline before every helper call / branch edge.
    fn flush(&mut self, fb: &mut FunctionBuilder) {
        if self.vst.is_empty() {
            return;
        }
        let k = self.vst.len() as i64;
        let len = self.stack_len(fb);
        let cap = self.stack_cap(fb);
        let need = fb.ins().iadd_imm(len, k);
        let fits = fb.ins().icmp(IntCC::UnsignedLessThanOrEqual, need, cap);
        let grow_b = fb.create_block();
        let cont_b = fb.create_block();
        fb.append_block_param(cont_b, self.ptr_ty);
        let sp0 = self.stack_ptr(fb);
        fb.ins().brif(fits, cont_b, &[BlockArg::Value(sp0)], grow_b, &[]);
        // Cold: grow, then reload the (possibly moved) data pointer.
        fb.switch_to_block(grow_b);
        let kc = fb.ins().iconst(types::I64, k);
        fb.ins().call(self.h.stack_reserve, &[self.vm, kc]);
        let lay = self.lay.expect("flush without probed layout");
        let sp1 = fb.ins().load(self.ptr_ty, fl(), self.vm, self.off_stack + lay.ptr_off);
        fb.ins().jump(cont_b, &[BlockArg::Value(sp1)]);
        fb.switch_to_block(cont_b);
        let sp = fb.block_params(cont_b)[0];
        self.sptr = Some(sp);
        self.scap = None; // unknown after a possible grow
        let base_off = fb.ins().ishl_imm(len, 4);
        let base = fb.ins().iadd(sp, base_off);
        let vals = std::mem::take(&mut self.vst);
        for (i, v) in vals.iter().enumerate() {
            fb.ins().store(fl(), v.w0, base, (i * 16) as i32);
            fb.ins().store(fl(), v.w1, base, (i * 16 + 8) as i32);
        }
        let new_len = fb.ins().iadd_imm(len, k);
        self.set_stack_len(fb, new_len);
    }

    /// `1 << tag ∈ mask` membership test; nonzero i64 iff in the mask.
    fn mask_test(&self, fb: &mut FunctionBuilder, mask: u64, tagv: ClValue) -> ClValue {
        let m = fb.ins().iconst(types::I64, mask as i64);
        let sh = fb.ins().ushr(m, tagv);
        fb.ins().band_imm(sh, 1)
    }

    fn tag_from_w0(&self, fb: &mut FunctionBuilder, w0: ClValue) -> ClValue {
        fb.ins().band_imm(w0, 0xff)
    }

    /// Runtime truthiness (i64 0/1) from a value's raw first word:
    /// falsy = tag==NIL || (tag==BOOL && bool_byte==0).
    fn truthy_from_w0(&self, fb: &mut FunctionBuilder, w0: ClValue) -> ClValue {
        let tag = self.tag_from_w0(fb, w0);
        let is_nil = fb.ins().icmp_imm(IntCC::Equal, tag, self.tags.nil as i64);
        let is_bool = fb.ins().icmp_imm(IntCC::Equal, tag, self.tags.bool_ as i64);
        let bbyte = fb.ins().ushr_imm(w0, 8);
        let bbit = fb.ins().band_imm(bbyte, 1);
        let bfalse = fb.ins().icmp_imm(IntCC::Equal, bbit, 0);
        let bool_falsy = fb.ins().band(is_bool, bfalse);
        let falsy = fb.ins().bor(is_nil, bool_falsy);
        let t = fb.ins().bxor_imm(falsy, 1);
        fb.ins().uextend(types::I64, t)
    }

    /// Status check after a status-returning helper: continue on CONTINUE,
    /// exit with the status otherwise. Switches to a fresh single-pred cont
    /// block (compile-time state survives; memory registers are the
    /// caller's responsibility via `invalidate_mem`).
    fn check_status(&mut self, fb: &mut FunctionBuilder, st: ClValue) {
        let cont = fb.create_block();
        let ok = fb.ins().icmp_imm(IntCC::Equal, st, T2_CONTINUE);
        fb.ins().brif(ok, cont, &[], self.exit, &[BlockArg::Value(st)]);
        fb.switch_to_block(cont);
    }

    /// Generic-helper emission for op `i`: flush, run the interpreter's own
    /// arm via `t2_op`, check the status, and continue the chain with
    /// canonical state. `clear_cache` because an arbitrary op may write
    /// locals (the enumerated inline ops handle their own slots precisely).
    ///
    /// FRAME-LITE bodies have no frame for `t2_op` to run against: the lite
    /// branch materializes at `i` and BAILs (this op — and the rest of the
    /// body — runs interpreted). Returns true when the emission TERMINATED
    /// the current block (the lite case); ops after a lite-generic are
    /// unreachable natively unless they are branch targets.
    fn emit_generic(&mut self, fb: &mut FunctionBuilder, i: usize) -> bool {
        self.flush(fb);
        if self.lite {
            self.emit_materialize_bail(fb, i);
            return true;
        }
        let opp = fb
            .ins()
            .iconst(self.ptr_ty, unsafe { self.code.as_ptr().add(i) } as i64);
        let pidxc = fb.ins().iconst(types::I64, self.pidx as i64);
        let ipc = fb.ins().iconst(types::I64, i as i64);
        let call = fb.ins().call(self.h.op, &[self.vm, opp, pidxc, ipc]);
        let st = fb.inst_results(call)[0];
        self.check_status(fb, st);
        self.invalidate_mem();
        self.clear_cache();
        false
    }

    /// Compute the slow-edge segment boundary starting at `from`:
    /// `(end, kind)` — `t2_resume` runs `[from, end)`.
    fn next_sync(&self, from: usize) -> (usize, SyncKind) {
        let n = self.code.len();
        let mut j = from;
        while j < n {
            if j > from && self.leader[j] {
                return (j, SyncKind::Leader(j));
            }
            if is_sync_op(&self.code[j]) {
                let kind = match self.code[j] {
                    Op::Jump(off) | Op::BreakLoop(off) | Op::NextLoop(off) => {
                        SyncKind::Uncond(jump_target(j, off))
                    }
                    Op::JumpIfFalse(off) => SyncKind::Cond(jump_target(j, off), j + 1),
                    Op::JumpIfArgGiven(_, off) | Op::JumpIfKwArgGiven(_, off) => {
                        SyncKind::Cond(jump_target(j, off), j + 1)
                    }
                    Op::Return => SyncKind::Return,
                    _ => unreachable!("is_sync_op mismatch"),
                };
                return (j + 1, kind);
            }
            j += 1;
        }
        (n, SyncKind::Return)
    }

    /// Fill a slow-edge block: materialize `snap`'s state, hand ops
    /// `[from, end)` to `t2_resume`, then rejoin native control flow. The
    /// CURRENT block must already be terminated (the guard's brif); this
    /// fills `fail_b` and leaves the builder positioned there-after —
    /// callers switch to their cont block next.
    ///
    /// FRAME-LITE bodies have no frame to resume into: the lite branch
    /// instead flushes the snapshot's virtual stack (making the operand
    /// temporaries real), MATERIALIZES the frame at `from` via
    /// `t2_lite_materialize` (the deferred frame push, with the current
    /// native locals), and returns `T2_BAIL` — the serve site's caller
    /// continues the fresh frame exactly like any interpreter push. The
    /// guarded op has had no effects (guards run first), so the interpreter
    /// re-runs it against canonical state: a mode switch, never a replay.
    fn fill_resume(&mut self, fb: &mut FunctionBuilder, fail_b: Block, snap: &CgSnap, from: usize) {
        if self.lite {
            let saved = self.snapshot();
            self.restore(snap.clone());
            fb.switch_to_block(fail_b);
            self.flush(fb);
            self.emit_materialize_bail(fb, from);
            self.restore(saved);
            return;
        }
        let saved = self.snapshot();
        self.restore(snap.clone());
        fb.switch_to_block(fail_b);
        self.flush(fb);
        let (end, kind) = self.next_sync(from);
        let pidxc = fb.ins().iconst(types::I64, self.pidx as i64);
        let fromc = fb.ins().iconst(types::I64, from as i64);
        let endc = fb.ins().iconst(types::I64, end as i64);
        let call = fb.ins().call(self.h.resume, &[self.vm, pidxc, fromc, endc]);
        let ret = fb.inst_results(call)[0];
        let st = fb.ins().band_imm(ret, 0xff);
        let ok = fb.ins().icmp_imm(IntCC::Equal, st, T2_CONTINUE);
        let disp = fb.create_block();
        fb.ins().brif(ok, disp, &[], self.exit, &[BlockArg::Value(st)]);
        fb.switch_to_block(disp);
        match kind {
            SyncKind::Leader(j) => {
                fb.ins().jump(self.blocks[j].expect("leader block"), &[]);
            }
            SyncKind::Uncond(t) => {
                fb.ins().jump(self.blocks[t].expect("target block"), &[]);
            }
            SyncKind::Cond(t, f) => {
                let ipres = fb.ins().ushr_imm(ret, 32);
                let is_t = fb.ins().icmp_imm(IntCC::Equal, ipres, t as i64);
                fb.ins().brif(
                    is_t,
                    self.blocks[t].expect("taken block"),
                    &[],
                    self.blocks[f].expect("fallthrough block"),
                    &[],
                );
            }
            SyncKind::Return => {
                // Unreachable in practice (`Return` through resume yields
                // DONE, not CONTINUE); BAIL is the safe fallback.
                let st2 = fb.ins().iconst(types::I64, T2_BAIL);
                fb.ins().return_(&[st2]);
            }
        }
        self.restore(saved);
    }

    /// Frame-lite: emit the materialize call + `return T2_BAIL` terminator
    /// into the CURRENT block. Virtual state must already be flushed; the
    /// interpreter continues the materialized frame at op `from`.
    fn emit_materialize_bail(&mut self, fb: &mut FunctionBuilder, from: usize) {
        let pidxc = fb.ins().iconst(types::I64, self.pidx as i64);
        let ipc = fb.ins().iconst(types::I64, from as i64);
        let argcc = fb.ins().iconst(types::I64, self.lite_argc as i64);
        let nlc = fb.ins().iconst(types::I64, self.cache.len() as i64);
        let n_pop = self.lite_n_pop.expect("lite n_pop");
        let trunc = self.lite_trunc.expect("lite trunc");
        let slot = self.lite_slot.expect("lite slot");
        if self.lite_blk {
            // LITE-BLOCK: the handle carries self; pass blk (id + 1) + ps.
            let blkid = self.lite_blkid.expect("lite blkid");
            let blk1 = fb.ins().iadd_imm(blkid, 1);
            let psc = fb.ins().iconst(types::I64, self.lite_ps as i64);
            fb.ins().call(
                self.h.lite_mat_blk,
                &[self.vm, pidxc, ipc, argcc, nlc, n_pop, trunc, slot, blk1, psc],
            );
            let st = fb.ins().iconst(types::I64, T2_BAIL);
            fb.ins().return_(&[st]);
            return;
        }
        let (sw0, sw1) = (
            self.self_w0.expect("lite self regs"),
            self.self_w1.expect("lite self regs"),
        );
        fb.ins().call(
            self.h.lite_mat,
            &[self.vm, pidxc, ipc, argcc, nlc, n_pop, trunc, slot, sw0, sw1],
        );
        let st = fb.ins().iconst(types::I64, T2_BAIL);
        fb.ins().return_(&[st]);
    }

    /// LITE t2_call emission: flush (recv/args become real, rooted stack
    /// slots), call the per-form helper, then branch on its status —
    /// 0 = served frameless (continue the chain; the LOCAL READ CACHE
    /// SURVIVES: no callee can write this activation's native spill slot —
    /// lite→lite callees have their own, the native serve families never
    /// touch it, and the materialized path never returns here), 1 = the
    /// frame was materialized (with any pending cascade): exit `T2_BAIL`.
    /// `args` are the helper-specific immediates AFTER (vm, ctx, meta).
    fn emit_lite_ext(
        &mut self,
        fb: &mut FunctionBuilder,
        i: usize,
        href: cranelift_codegen::ir::FuncRef,
        args: &[i64],
    ) -> bool {
        let Some(ctx) = self.lite_ctx else {
            // No ctx slot was laid down (nil?-fusion-only bodies): keep
            // the wave-4 materialize behaviour.
            return self.emit_generic(fb, i);
        };
        self.flush(fb);
        let meta = (self.pidx as u64)
            | ((self.cache.len() as u64) << 32)
            | ((self.lite_argc as u64) << 40)
            | ((self.lite_ps as u64) << 44);
        let metac = fb.ins().iconst(types::I64, meta as i64);
        let mut call_args = vec![self.vm, ctx, metac];
        for &a in args {
            call_args.push(fb.ins().iconst(types::I64, a));
        }
        let ipc = fb.ins().iconst(types::I64, i as i64);
        call_args.push(ipc);
        let call = fb.ins().call(href, &call_args);
        let st = fb.inst_results(call)[0];
        self.invalidate_mem();
        let ok = fb.ins().icmp_imm(IntCC::Equal, st, 0);
        let cont = fb.create_block();
        let bailc = fb.ins().iconst(types::I64, T2_BAIL);
        fb.ins().brif(ok, cont, &[], self.exit, &[BlockArg::Value(bailc)]);
        fb.switch_to_block(cont);
        false
    }

    /// One guard: continue on `ok`, take the shared per-op resume edge
    /// otherwise. `fail_b` is created+filled on first use; `snap` is the
    /// op-entry snapshot.
    fn guard(
        &mut self,
        fb: &mut FunctionBuilder,
        ok: ClValue,
        fail_b: &mut Option<Block>,
        snap: &CgSnap,
        from: usize,
    ) {
        let cont = fb.create_block();
        let need_fill = fail_b.is_none();
        let fbk = *fail_b.get_or_insert_with(|| fb.create_block());
        fb.ins().brif(ok, cont, &[], fbk, &[]);
        if need_fill {
            self.fill_resume(fb, fbk, snap, from);
        }
        fb.switch_to_block(cont);
    }

    /// Jump target for a branch edge, routed through the shared poll block
    /// when the edge is backward (loop back-edge: signal/interrupt/fuel
    /// poll). Poll blocks are filled after the main loop.
    fn edge_block(&mut self, fb: &mut FunctionBuilder, from: usize, target: usize) -> Block {
        if target > from {
            return self.blocks[target].expect("forward target block");
        }
        if let Some(b) = self.poll_blocks.get(&target) {
            return *b;
        }
        let b = fb.create_block();
        self.poll_blocks.insert(target, b);
        b
    }

    /// Fill all pending poll blocks (called once, after the main loop; every
    /// poll block is entered with canonical state). FRAME-LITE bodies can't
    /// run `t2_poll` (it stamps `frame.ip` and `check_fuel` can raise), so a
    /// fired gate materializes the frame at the branch target and BAILs —
    /// the dispatch loop head then owns signal/interrupt delivery, and a
    /// fuel-capped run continues the (now framed) loop with the
    /// interpreter's own per-op charging.
    fn fill_poll_blocks(&mut self, fb: &mut FunctionBuilder, interrupt_addr: usize) {
        let targets: Vec<(usize, Block)> = self.poll_blocks.iter().map(|(t, b)| (*t, *b)).collect();
        for (target, b) in targets {
            fb.switch_to_block(b);
            let g1 = fb.ins().load(types::I8, fl(), self.vm, self.off_signals);
            let g2 = fb.ins().load(types::I8, fl(), self.vm, self.off_poll_flags);
            let ia = fb.ins().iconst(self.ptr_ty, interrupt_addr as i64);
            let g3 = fb.ins().load(types::I8, fl(), ia, 0);
            let g12 = fb.ins().bor(g1, g2);
            let g = fb.ins().bor(g12, g3);
            let do_b = fb.create_block();
            let tgt = self.blocks[target].expect("poll target block");
            fb.ins().brif(g, do_b, &[], tgt, &[]);
            fb.switch_to_block(do_b);
            if self.lite {
                // Poll blocks are entered with canonical (fully flushed)
                // state — materialize directly.
                self.emit_materialize_bail(fb, target);
                continue;
            }
            let ipc = fb.ins().iconst(types::I64, target as i64);
            let call = fb.ins().call(self.h.poll, &[self.vm, ipc]);
            let st = fb.inst_results(call)[0];
            let ok = fb.ins().icmp_imm(IntCC::Equal, st, T2_CONTINUE);
            fb.ins().brif(ok, tgt, &[], self.exit, &[BlockArg::Value(st)]);
        }
    }

    // --- VVal constructors ---

    fn vv_int(&self, fb: &mut FunctionBuilder, w1: ClValue) -> VVal {
        let w0 = fb.ins().iconst(types::I64, self.tags.int as i64);
        VVal { w0, w1, tag: Some(self.tags.int), bit: None, truthy: Some(true) }
    }

    fn vv_bool_bit(&self, fb: &mut FunctionBuilder, bit_any: ClValue) -> VVal {
        // Normalize the 0/1 to i64 first (icmp yields i8).
        let bit = if fb.func.dfg.value_type(bit_any) == types::I64 {
            bit_any
        } else {
            fb.ins().uextend(types::I64, bit_any)
        };
        let sh = fb.ins().ishl_imm(bit, 8);
        let tagc = fb.ins().iconst(types::I64, self.tags.bool_ as i64);
        let w0 = fb.ins().bor(tagc, sh);
        let w1 = fb.ins().iconst(types::I64, 0);
        VVal { w0, w1, tag: Some(self.tags.bool_), bit: Some(bit), truthy: None }
    }

    /// Pop a 2-operand op's inputs: `(lhs, rhs, n_real)`. Virtual entries
    /// are popped from the compile-time vstack (the caller snapshots
    /// beforehand for its slow edge); real entries are PEEKED — the caller
    /// consumes them with `consume_reals` after its guards pass.
    fn take2(&mut self, fb: &mut FunctionBuilder) -> (Operand, Operand, i64) {
        let (rhs, r_real) = match self.vst.pop() {
            Some(v) => (Operand { w0: v.w0, w1: v.w1, tag: v.tag }, 0i64),
            None => {
                let a = self.stack_slot_addr(fb, 0);
                let w0 = fb.ins().load(types::I64, fl(), a, 0);
                let w1 = fb.ins().load(types::I64, fl(), a, 8);
                (Operand { w0, w1, tag: None }, 1i64)
            }
        };
        let (lhs, l_real) = match self.vst.pop() {
            Some(v) => (Operand { w0: v.w0, w1: v.w1, tag: v.tag }, 0i64),
            None => {
                let a = self.stack_slot_addr(fb, r_real);
                let w0 = fb.ins().load(types::I64, fl(), a, 0);
                let w1 = fb.ins().load(types::I64, fl(), a, 8);
                (Operand { w0, w1, tag: None }, 1i64)
            }
        };
        (lhs, rhs, r_real + l_real)
    }

    fn consume_reals(&mut self, fb: &mut FunctionBuilder, n: i64) {
        if n == 0 {
            return;
        }
        let len = self.stack_len(fb);
        let nl = fb.ins().iadd_imm(len, -n);
        self.set_stack_len(fb, nl);
    }

    /// Emit a tag guard for an operand unless the tag is compile-known.
    /// Returns false when the operand is KNOWN to violate the guard (the
    /// caller should fall back to the generic helper for the whole op).
    fn guard_tag(
        &mut self,
        fb: &mut FunctionBuilder,
        o: &Operand,
        want: u8,
        fail_b: &mut Option<Block>,
        snap: &CgSnap,
        from: usize,
    ) -> bool {
        match o.tag {
            Some(t) if t == want => true,
            Some(_) => false,
            None => {
                let tag = self.tag_from_w0(fb, o.w0);
                let ok = fb.ins().icmp_imm(IntCC::Equal, tag, want as i64);
                self.guard(fb, ok, fail_b, snap, from);
                true
            }
        }
    }
}

/// The main emission loop. Returns false when the body is malformed (falls
/// off the end without a terminator) — the caller declines the compile.
///
/// `lite_argc`: `Some(argc)` emits the wave-4 FRAME-LITE variant — the
/// function signature becomes `(vm, self_w0, self_w1, n_pop) -> status`, no
/// frame exists, the callee's `argc` args are read off the operand-stack
/// top into a native spill slot (the canonical local store while
/// frameless), and every edge the lite mode can't serve materializes the
/// frame + BAILs (see `t2_lite_materialize`). Requires `inline_on` (the
/// probed Vec layout) — the caller guarantees it.
#[allow(clippy::too_many_arguments)]
fn emit_body(
    fb: &mut FunctionBuilder,
    h: &HelperRefs,
    proto: &Proto,
    proto_idx: usize,
    code: &[Op],
    n: usize,
    leader: &[bool],
    ptr_ty: Type,
    t2ctx: &T2Ctx,
    inline_on: bool,
    cacheable: bool,
    lite_mode: LiteMode,
) -> bool {
    let tags = t2_tags();
    let entry = fb.create_block();
    fb.append_block_params_for_function_params(entry);
    let exit = fb.create_block();
    fb.append_block_param(exit, types::I64);
    let blocks: Vec<Option<Block>> = leader
        .iter()
        .map(|&l| if l { Some(fb.create_block()) } else { None })
        .collect();

    fb.switch_to_block(entry);
    let vm = fb.block_params(entry)[0];
    let lite = !matches!(lite_mode, LiteMode::Off);
    let lite_blk = matches!(lite_mode, LiteMode::Block(..));
    // The entry's arg count: bound params for a block, plain argc for a
    // method (n_pop additionally counts the recv slot at runtime there).
    let (lite_ps, lite_np) = match lite_mode {
        LiteMode::Off => (0u16, 0u16),
        LiteMode::Method(a) => (0, a),
        LiteMode::Block(ps, np, _) => (ps, np),
    };

    let mut cg = Cg {
        ptr_ty,
        vm,
        tags,
        lay: if inline_on { vec_layout() } else { None },
        t2ctx,
        h,
        pidx: proto_idx,
        code,
        leader,
        blocks,
        exit,
        inline_on,
        cacheable,
        nocall: t2ctx.nocall,
        lite,
        lite_slot: None,
        lite_ctx: None,
        lite_trunc: None,
        lite_n_pop: None,
        lite_argc: lite_np,
        lite_blk,
        lite_ps,
        lite_blkid: None,
        poll_blocks: crate::intern::FxHashMap::default(),
        vst: Vec::new(),
        cache: vec![None; proto.n_locals as usize],
        sptr: None,
        slen: None,
        scap: None,
        aptr: None,
        ebase16: None,
        self_w0: None,
        self_w1: None,
        scratch: None,
        off_stack: offset_of!(crate::vm::Vm, stack) as i32,
        off_arena: offset_of!(crate::vm::Vm, locals_arena) as i32,
        off_signals: offset_of!(crate::vm::Vm, control_signals) as i32,
        off_poll_flags: offset_of!(crate::vm::Vm, t2_poll_flags) as i32,
        off_reopen: offset_of!(crate::vm::Vm, prim_reopen_mask) as i32,
    };

    if lite {
        // FRAME-LITE entry: params are (vm, self_w0, self_w1, n_pop) —
        // lite-BLOCK entries carry the BlockHandle id in the 4th slot
        // instead (their n_pop = n_params is a compile-time constant).
        // The args are the operand stack's top slots — left IN PLACE
        // (rooted, owned by the stack until return/materialize); their raw
        // words are copied into the native spill slot as borrowing views:
        // method args at slots [0, argc), block args at
        // [param_start, param_start + n_params). Non-arg OWN-region slots
        // initialize to Nil, mirroring the interpreter's binder (a block's
        // outer prefix `[0, param_start)` is never spilled — those slots
        // route through the canonical cells).
        let params: Vec<ClValue> = fb.block_params(entry).to_vec();
        let (sw0, sw1) = (params[1], params[2]);
        cg.self_w0 = Some(sw0);
        cg.self_w1 = Some(sw1);
        let n_pop = if lite_blk {
            cg.lite_blkid = Some(params[3]);
            fb.ins().iconst(types::I64, lite_np as i64)
        } else {
            params[3]
        };
        cg.lite_n_pop = Some(n_pop);
        let ss = fb.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 32, 3));
        cg.scratch = Some(fb.ins().stack_addr(ptr_ty, ss, 0));
        let n_locals = proto.n_locals.max(1) as u32;
        let ls = fb.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            n_locals * 16,
            3,
        ));
        let lsa = fb.ins().stack_addr(ptr_ty, ls, 0);
        cg.lite_slot = Some(lsa);
        let sp = cg.stack_ptr(fb);
        let len = cg.stack_len(fb);
        cg.lite_trunc = Some(fb.ins().isub(len, n_pop));
        let argc = lite_np;
        let arg_base = lite_ps as i32; // 0 in method mode
        if argc > 0 {
            // &stack[len] — args at negative offsets from it.
            let end_off = fb.ins().ishl_imm(len, 4);
            let sp_end = fb.ins().iadd(sp, end_off);
            for i in 0..argc as i32 {
                let off = -16 * (argc as i32 - i);
                let w0 = fb.ins().load(types::I64, fl(), sp_end, off);
                let w1 = fb.ins().load(types::I64, fl(), sp_end, off + 8);
                fb.ins().store(fl(), w0, lsa, (arg_base + i) * 16);
                fb.ins().store(fl(), w1, lsa, (arg_base + i) * 16 + 8);
            }
        }
        if (proto.n_locals as i32) > arg_base + argc as i32 {
            let nil0 = fb.ins().iconst(types::I64, tags.nil as i64);
            let zero = fb.ins().iconst(types::I64, 0);
            for s in (arg_base + argc as i32)..proto.n_locals as i32 {
                fb.ins().store(fl(), nil0, lsa, s * 16);
                fb.ins().store(fl(), zero, lsa, s * 16 + 8);
            }
        }
        // LITE t2_call caller-context slot: bodies containing admitted
        // call/const ops (the `nil?`-fusion Call doesn't count — its
        // empty-vst fallback keeps the wave-4 materialize path) get a
        // 6-word slot the helpers read: [slot_addr, trunc, n_pop,
        // self_w0, self_w1, blk]. Filled ONCE here (the values are entry
        // constants for the whole activation). `blk` = block_id + 1 for a
        // lite-block caller, 0 for a method — it routes the cascade's
        // deferred push to the right frame shape.
        let has_ext = code.iter().any(|op| match op {
            Op::Call(name, _, _) => name.0 != t2ctx.sym_nil_q,
            Op::CallNoRecv(..)
            | Op::LoadLocalCall(..)
            | Op::LoadConstChain(_)
            | Op::LoadConst(_) => true,
            _ => false,
        });
        if has_ext {
            let cslot =
                fb.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 48, 3));
            let ca = fb.ins().stack_addr(ptr_ty, cslot, 0);
            let trunc = cg.lite_trunc.expect("lite trunc");
            fb.ins().store(fl(), lsa, ca, 0);
            fb.ins().store(fl(), trunc, ca, 8);
            fb.ins().store(fl(), n_pop, ca, 16);
            fb.ins().store(fl(), sw0, ca, 24);
            fb.ins().store(fl(), sw1, ca, 32);
            let blk = if let Some(blkid) = cg.lite_blkid {
                fb.ins().iadd_imm(blkid, 1)
            } else {
                fb.ins().iconst(types::I64, 0)
            };
            fb.ins().store(fl(), blk, ca, 40);
            cg.lite_ctx = Some(ca);
        }
    } else if cg.inline_on {
        // Entry sequence: one info call fills the scratch slot with the
        // frame's Locals::Stack arena base (-1 for Shared) + the raw self
        // words. The cacheable path double-guards at runtime: a Shared
        // frame (shouldn't happen for a creates_block==false method proto;
        // possible if a future serving site pushes differently) BAILS with
        // ip=0 — the interpreter runs the body, correctness unharmed.
        let ss = fb.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 32, 3));
        let out = fb.ins().stack_addr(ptr_ty, ss, 0);
        cg.scratch = Some(out);
        fb.ins().call(h.entry_info, &[vm, out]);
        let sw0 = fb.ins().load(types::I64, fl(), out, 8);
        let sw1 = fb.ins().load(types::I64, fl(), out, 16);
        cg.self_w0 = Some(sw0);
        cg.self_w1 = Some(sw1);
        if cg.cacheable {
            let base = fb.ins().load(types::I64, fl(), out, 0);
            let ok = fb.ins().icmp_imm(IntCC::NotEqual, base, -1);
            let cont = fb.create_block();
            let bail = fb.create_block();
            fb.ins().brif(ok, cont, &[], bail, &[]);
            fb.switch_to_block(bail);
            let st = fb.ins().iconst(types::I64, T2_BAIL);
            fb.ins().return_(&[st]);
            fb.switch_to_block(cont);
            cg.ebase16 = Some(fb.ins().ishl_imm(base, 4));
        }
    }
    fb.ins().jump(cg.blocks[0].expect("entry leader"), &[]);

    let mut terminated = true;
    for i in 0..n {
        if let Some(b) = cg.blocks[i] {
            if !terminated {
                cg.flush(fb);
                fb.ins().jump(b, &[]);
            }
            fb.switch_to_block(b);
            terminated = false;
            cg.reset_block_state();
        }
        if terminated {
            continue; // unreachable op between a terminator and the next leader
        }
        terminated = emit_op(&mut cg, fb, i);
    }
    if !terminated {
        return false; // no trailing terminator: refuse
    }
    cg.fill_poll_blocks(fb, t2ctx.interrupt_addr);
    fb.switch_to_block(exit);
    let st = fb.block_params(exit)[0];
    fb.ins().return_(&[st]);
    fb.seal_all_blocks();
    true
}

/// Emit one op on the fast chain. Returns true when the op terminated the
/// chain (branch / return).
fn emit_op(cg: &mut Cg, fb: &mut FunctionBuilder, i: usize) -> bool {
    let vm = cg.vm;
    let op = cg.code[i];
    match op {
        // ------------------------------------------------------------------
        // Branches / return: segment enders.
        // ------------------------------------------------------------------
        Op::Jump(off) => {
            cg.flush(fb);
            let tgt = jump_target(i, off);
            let b = cg.edge_block(fb, i, tgt);
            fb.ins().jump(b, &[]);
            true
        }
        Op::JumpIfFalse(off) => {
            let tgt = jump_target(i, off);
            if let Some(cond) = cg.vst.pop() {
                cg.flush(fb);
                // Compile-known truthiness → unconditional edge.
                let known = if let Some(t) = cond.truthy {
                    Some(t)
                } else if let Some(tag) = cond.tag {
                    if tag == cg.tags.nil {
                        Some(false)
                    } else if tag != cg.tags.bool_ {
                        Some(true)
                    } else {
                        None
                    }
                } else {
                    None
                };
                match known {
                    Some(true) => {
                        let b = cg.blocks[i + 1].expect("fallthrough block");
                        fb.ins().jump(b, &[]);
                    }
                    Some(false) => {
                        let b = cg.edge_block(fb, i, tgt);
                        fb.ins().jump(b, &[]);
                    }
                    None => {
                        let bit = match cond.bit {
                            Some(b) => b,
                            None => cg.truthy_from_w0(fb, cond.w0),
                        };
                        let tb = cg.edge_block(fb, i, tgt);
                        let fbk = cg.blocks[i + 1].expect("fallthrough block");
                        fb.ins().brif(bit, fbk, &[], tb, &[]);
                    }
                }
            } else if cg.inline_on {
                // Real condition: inline pop+truthiness for trivial tags,
                // helper for the rest; both paths merge on a 0/1 bit.
                let a = cg.stack_slot_addr(fb, 0);
                let w0 = fb.ins().load(types::I64, fl(), a, 0);
                let tag = cg.tag_from_w0(fb, w0);
                let triv = cg.mask_test(fb, cg.tags.trivial_mask, tag);
                let fast_b = fb.create_block();
                let slow_b = fb.create_block();
                let merge = fb.create_block();
                fb.append_block_param(merge, types::I64);
                fb.ins().brif(triv, fast_b, &[], slow_b, &[]);
                fb.switch_to_block(fast_b);
                let len = cg.stack_len(fb);
                let nl = fb.ins().iadd_imm(len, -1);
                cg.set_stack_len(fb, nl);
                let bit1 = cg.truthy_from_w0(fb, w0);
                fb.ins().jump(merge, &[BlockArg::Value(bit1)]);
                fb.switch_to_block(slow_b);
                let call = fb.ins().call(cg.h.pop_truthy, &[vm]);
                let bit2 = fb.inst_results(call)[0];
                fb.ins().jump(merge, &[BlockArg::Value(bit2)]);
                fb.switch_to_block(merge);
                cg.invalidate_mem();
                let bit = fb.block_params(merge)[0];
                let tb = cg.edge_block(fb, i, tgt);
                let fbk = cg.blocks[i + 1].expect("fallthrough block");
                fb.ins().brif(bit, fbk, &[], tb, &[]);
            } else {
                let call = fb.ins().call(cg.h.pop_truthy, &[vm]);
                let truthy = fb.inst_results(call)[0];
                let tb = cg.edge_block(fb, i, tgt);
                let fbk = cg.blocks[i + 1].expect("fallthrough block");
                fb.ins().brif(truthy, fbk, &[], tb, &[]);
            }
            true
        }
        Op::JumpIfArgGiven(slot, off) => {
            cg.flush(fb);
            let s = fb.ins().iconst(types::I64, slot as i64);
            let call = fb.ins().call(cg.h.arg_given, &[vm, s]);
            let given = fb.inst_results(call)[0];
            let tgt = jump_target(i, off);
            let tb = cg.edge_block(fb, i, tgt);
            let fbk = cg.blocks[i + 1].expect("fallthrough block");
            fb.ins().brif(given, tb, &[], fbk, &[]);
            true
        }
        Op::JumpIfKwArgGiven(kw_idx, off) => {
            cg.flush(fb);
            let s = fb.ins().iconst(types::I64, kw_idx as i64);
            let call = fb.ins().call(cg.h.kwarg_given, &[vm, s]);
            let given = fb.inst_results(call)[0];
            let tgt = jump_target(i, off);
            let tb = cg.edge_block(fb, i, tgt);
            let fbk = cg.blocks[i + 1].expect("fallthrough block");
            fb.ins().brif(given, tb, &[], fbk, &[]);
            true
        }
        Op::BreakLoop(off) | Op::NextLoop(off) => {
            // Run the step arm (loop bookkeeping + its own ip retarget),
            // then take the SAME edge natively.
            cg.flush(fb);
            let opp = fb
                .ins()
                .iconst(cg.ptr_ty, unsafe { cg.code.as_ptr().add(i) } as i64);
            let pidxc = fb.ins().iconst(types::I64, cg.pidx as i64);
            let ipc = fb.ins().iconst(types::I64, i as i64);
            let call = fb.ins().call(cg.h.op, &[vm, opp, pidxc, ipc]);
            let st = fb.inst_results(call)[0];
            cg.check_status(fb, st);
            cg.invalidate_mem();
            let tgt = jump_target(i, off);
            let b = cg.edge_block(fb, i, tgt);
            fb.ins().jump(b, &[]);
            true
        }
        Op::Return => {
            if cg.lite {
                // Frame-lite return: replace recv/args (and any operand
                // temporaries) with the return value — no frame to pop, no
                // `$~`/`$!`/aux/arena discipline to run (admission
                // guarantees none has observable effects).
                let trunc = cg.lite_trunc.expect("lite trunc");
                let st = if let Some(v) = cg.vst.pop() {
                    // Remaining virtuals below the return value are trivial
                    // by invariant — discarding compiles to nothing.
                    cg.vst.clear();
                    let call = fb.ins().call(cg.h.lite_ret_v, &[vm, v.w0, v.w1, trunc]);
                    fb.inst_results(call)[0]
                } else {
                    let call = fb.ins().call(cg.h.lite_ret_s, &[vm, trunc]);
                    fb.inst_results(call)[0]
                };
                fb.ins().return_(&[st]);
                return true;
            }
            if cg.nocall {
                let opp = fb
                    .ins()
                    .iconst(cg.ptr_ty, unsafe { cg.code.as_ptr().add(i) } as i64);
                let pidxc = fb.ins().iconst(types::I64, cg.pidx as i64);
                let ipc = fb.ins().iconst(types::I64, i as i64);
                cg.flush(fb);
                let call = fb.ins().call(cg.h.op, &[vm, opp, pidxc, ipc]);
                let st = fb.inst_results(call)[0];
                fb.ins().return_(&[st]);
                return true;
            }
            let pidxc = fb.ins().iconst(types::I64, cg.pidx as i64);
            let ipc = fb.ins().iconst(types::I64, i as i64);
            if let Some(v) = cg.vst.pop() {
                // Remaining virtuals below the return value would be
                // dropped by the helper's truncate; they are trivial by
                // invariant, so discarding them compiles to nothing.
                cg.vst.clear();
                let call = fb
                    .ins()
                    .call(cg.h.return_v, &[vm, v.w0, v.w1, pidxc, ipc]);
                let st = fb.inst_results(call)[0];
                fb.ins().return_(&[st]);
            } else {
                let call = fb.ins().call(cg.h.ret, &[vm, pidxc, ipc]);
                let st = fb.inst_results(call)[0];
                fb.ins().return_(&[st]);
            }
            true
        }
        // ------------------------------------------------------------------
        // Wave-2 IC-fast call family (chain-inline: the local read cache
        // survives — callees cannot write a Locals::Stack frame's slots).
        // ------------------------------------------------------------------
        Op::Call(name, argc, cid)
            if !cg.nocall && (argc <= T2_CALL_MAX_ARGC || (cg.lite && (argc as u16) <= LITE_MAX_ARGC)) =>
        {
            // `x.nil?` on a virtual receiver: the interpreter serves this
            // via try_fast_primitive's universal arm for Int/Float/Sym/
            // Bool/Nil receivers whenever no primitive class was reopened
            // (`prim_reopen_mask == 0`) — same gates, inlined.
            if cg.inline_on && argc == 0 && name.0 == cg.t2ctx.sym_nil_q && !cg.vst.is_empty() {
                let snap = cg.snapshot();
                let recv = cg.vst.pop().expect("checked nonempty");
                let mut fail_b: Option<Block> = None;
                let static_in_mask = recv.tag.map(|t| cg.tags.nilq_mask & (1 << t) != 0);
                if static_in_mask != Some(false) {
                    // reopen gate (byte == 0)
                    let reopen = fb.ins().load(types::I8, fl(), vm, cg.off_reopen);
                    let gate_ok = fb.ins().icmp_imm(IntCC::Equal, reopen, 0);
                    cg.guard(fb, gate_ok, &mut fail_b, &snap, i);
                    let bit = if let Some(t) = recv.tag {
                        // statically in the mask; answer is a constant
                        let b = (t == cg.tags.nil) as i64;
                        fb.ins().iconst(types::I64, b)
                    } else {
                        let tag = cg.tag_from_w0(fb, recv.w0);
                        let in_mask = cg.mask_test(fb, cg.tags.nilq_mask, tag);
                        let ok = fb.ins().icmp_imm(IntCC::NotEqual, in_mask, 0);
                        cg.guard(fb, ok, &mut fail_b, &snap, i);
                        let isnil =
                            fb.ins().icmp_imm(IntCC::Equal, tag, cg.tags.nil as i64);
                        fb.ins().uextend(types::I64, isnil)
                    };
                    let v = cg.vv_bool_bit(fb, bit);
                    cg.vst.push(v);
                    return false;
                }
                // Statically outside the fast set (e.g. an Object): plain
                // call path below.
                cg.restore(snap);
            }
            if cg.lite {
                // LITE t2_call: the frameless call helper — serve or
                // materialize (a `nil?` shape the fusion above couldn't
                // take goes through the same helper when a ctx exists).
                return cg.emit_lite_ext(
                    fb,
                    i,
                    cg.h.lite_call_ex,
                    &[name.0 as i64, argc as i64, cid as i64],
                );
            }
            cg.flush(fb);
            let nc = fb.ins().iconst(types::I64, name.0 as i64);
            let a = fb.ins().iconst(types::I64, argc as i64);
            let c = fb.ins().iconst(types::I64, cid as i64);
            let ipc = fb.ins().iconst(types::I64, i as i64);
            let call = fb.ins().call(cg.h.call, &[vm, nc, a, c, ipc]);
            let st = fb.inst_results(call)[0];
            cg.check_status(fb, st);
            cg.invalidate_mem();
            false
        }
        Op::CallNoRecv(name, argc, cid)
            if !cg.nocall && (argc <= T2_CALL_MAX_ARGC || (cg.lite && (argc as u16) <= LITE_MAX_ARGC)) =>
        {
            if cg.lite {
                return cg.emit_lite_ext(
                    fb,
                    i,
                    cg.h.lite_call_ns,
                    &[name.0 as i64, argc as i64, cid as i64],
                );
            }
            cg.flush(fb);
            let nc = fb.ins().iconst(types::I64, name.0 as i64);
            let a = fb.ins().iconst(types::I64, argc as i64);
            let c = fb.ins().iconst(types::I64, cid as i64);
            let ipc = fb.ins().iconst(types::I64, i as i64);
            let call = fb.ins().call(cg.h.call_norecv, &[vm, nc, a, c, ipc]);
            let st = fb.inst_results(call)[0];
            cg.check_status(fb, st);
            cg.invalidate_mem();
            false
        }
        Op::LoadLocalCall(slot, name, cid) if !cg.nocall => {
            if cg.lite {
                return cg.emit_lite_ext(
                    fb,
                    i,
                    cg.h.lite_call_local,
                    &[slot as i64, name.0 as i64, cid as i64],
                );
            }
            cg.flush(fb);
            let s = fb.ins().iconst(types::I64, slot as i64);
            let nc = fb.ins().iconst(types::I64, name.0 as i64);
            let c = fb.ins().iconst(types::I64, cid as i64);
            let ipc = fb.ins().iconst(types::I64, i as i64);
            let call = fb.ins().call(cg.h.call_local, &[vm, s, nc, c, ipc]);
            let st = fb.inst_results(call)[0];
            cg.check_status(fb, st);
            cg.invalidate_mem();
            false
        }
        // `Op::Super(name, argc, cid)` — lean serve (campaign P5a): the
        // step arm's drain + `super_call_with_lifecycle_noop` behind the
        // call family's own boundary (prologue fuel/ip + `t2_finish`)
        // instead of the generic `t2_op`. Same discipline as the call
        // arms: the callee cannot write this frame's `Locals::Stack`
        // slots, so the local read cache survives. `!cg.nocall` keeps
        // the debug knob's "route every call through the interpreter"
        // contract; lite bodies never admit Super (default arm bails).
        Op::Super(name, argc, cid) if !cg.nocall && !cg.lite => {
            cg.flush(fb);
            let nc = fb.ins().iconst(types::I64, name.0 as i64);
            let a = fb.ins().iconst(types::I64, argc as i64);
            let c = fb.ins().iconst(types::I64, cid as i64);
            let ipc = fb.ins().iconst(types::I64, i as i64);
            let call = fb.ins().call(cg.h.super_, &[vm, nc, a, c, ipc]);
            let st = fb.inst_results(call)[0];
            cg.check_status(fb, st);
            cg.invalidate_mem();
            false
        }
        // LITE t2_call: the IC-cached bare-constant read (frameless on a
        // cache hit; cold/invalidated → materialize + interpreted refill).
        Op::LoadConstChain(ci) if cg.lite => {
            cg.emit_lite_ext(fb, i, cg.h.lite_const, &[ci as i64])
        }
        // FRAMED IC-hit const reads (ADR 0037 tail): serve the
        // interpreter's own inline constant caches without the generic
        // `step()` round-trip; a miss runs the full arm (autoload /
        // const_missing / NameError / refill) via `t2_const_miss`.
        Op::LoadConstChain(ci) if !cg.nocall => {
            cg.flush(fb);
            let cic = fb.ins().iconst(types::I64, ci as i64);
            let pidxc = fb.ins().iconst(types::I64, cg.pidx as i64);
            let ipc = fb.ins().iconst(types::I64, i as i64);
            let call = fb.ins().call(cg.h.const_chain, &[vm, cic, pidxc, ipc]);
            let st = fb.inst_results(call)[0];
            cg.check_status(fb, st);
            cg.invalidate_mem();
            false
        }
        Op::LoadConst(sym) if cg.lite => {
            cg.emit_lite_ext(fb, i, cg.h.lite_const_flat, &[sym.0 as i64])
        }
        Op::LoadConst(sym) if !cg.nocall => {
            cg.flush(fb);
            let symc = fb.ins().iconst(types::I64, sym.0 as i64);
            let ipc = fb.ins().iconst(types::I64, i as i64);
            let call = fb.ins().call(cg.h.const_flat, &[vm, symc, ipc]);
            let st = fb.inst_results(call)[0];
            cg.check_status(fb, st);
            cg.invalidate_mem();
            false
        }
        // Fresh string-literal push (both tiers): raise-free, GC-heap-free
        // (`Rc`-backed `Value::Str`) — a plain infallible helper call, no
        // status. The interpreter arm's fresh-allocation semantics are
        // preserved (each execution pushes an independent string).
        Op::LoadConstStr(id) if !cg.nocall => {
            cg.flush(fb);
            let symc = fb.ins().iconst(types::I64, id.0 as i64);
            let pidxc = fb.ins().iconst(types::I64, cg.pidx as i64);
            fb.ins().call(cg.h.push_const_str, &[vm, symc, pidxc]);
            cg.invalidate_mem();
            false
        }
        // ------------------------------------------------------------------
        // Wave-5 block family: block-passing calls + yield through their
        // dedicated helpers (block-form IC fast path / do_yield), so a
        // compiled caller reaches a compiled callee/block without the
        // generic step() round-trip.
        // ------------------------------------------------------------------
        Op::CallBlock(name, argc, cid) if !cg.nocall => {
            cg.flush(fb);
            let nc = fb.ins().iconst(types::I64, name.0 as i64);
            let a = fb.ins().iconst(types::I64, argc as i64);
            let c = fb.ins().iconst(types::I64, cid as i64);
            let ipc = fb.ins().iconst(types::I64, i as i64);
            let call = fb.ins().call(cg.h.call_block, &[vm, nc, a, c, ipc]);
            let st = fb.inst_results(call)[0];
            cg.check_status(fb, st);
            cg.invalidate_mem();
            false
        }
        Op::CallNoRecvBlock(name, argc, cid) if !cg.nocall => {
            cg.flush(fb);
            let nc = fb.ins().iconst(types::I64, name.0 as i64);
            let a = fb.ins().iconst(types::I64, argc as i64);
            let c = fb.ins().iconst(types::I64, cid as i64);
            let ipc = fb.ins().iconst(types::I64, i as i64);
            let call = fb.ins().call(cg.h.call_norecv_block, &[vm, nc, a, c, ipc]);
            let st = fb.inst_results(call)[0];
            cg.check_status(fb, st);
            cg.invalidate_mem();
            false
        }
        Op::Yield(n_args) if !cg.nocall => {
            cg.flush(fb);
            let a = fb.ins().iconst(types::I64, n_args as i64);
            let ipc = fb.ins().iconst(types::I64, i as i64);
            let call = fb.ins().call(cg.h.yield_, &[vm, a, ipc]);
            let st = fb.inst_results(call)[0];
            cg.check_status(fb, st);
            cg.invalidate_mem();
            false
        }
        Op::ApplyYield if !cg.nocall => {
            cg.flush(fb);
            let a = fb.ins().iconst(types::I64, -1);
            let ipc = fb.ins().iconst(types::I64, i as i64);
            let call = fb.ins().call(cg.h.yield_, &[vm, a, ipc]);
            let st = fb.inst_results(call)[0];
            cg.check_status(fb, st);
            cg.invalidate_mem();
            false
        }
        // ------------------------------------------------------------------
        // Inline literals: free until materialization.
        // ------------------------------------------------------------------
        Op::LoadConstInt(v) if cg.inline_on => {
            let w1 = fb.ins().iconst(types::I64, v);
            let vv = cg.vv_int(fb, w1);
            cg.vst.push(vv);
            false
        }
        Op::LoadConstFloat(f) if cg.inline_on => {
            let w0 = fb.ins().iconst(types::I64, cg.tags.float as i64);
            let w1 = fb.ins().iconst(types::I64, f.to_bits() as i64);
            cg.vst.push(VVal { w0, w1, tag: Some(cg.tags.float), bit: None, truthy: Some(true) });
            false
        }
        Op::LoadNil if cg.inline_on => {
            let w0 = fb.ins().iconst(types::I64, cg.tags.nil as i64);
            let w1 = fb.ins().iconst(types::I64, 0);
            cg.vst.push(VVal { w0, w1, tag: Some(cg.tags.nil), bit: None, truthy: Some(false) });
            false
        }
        Op::LoadTrue | Op::LoadFalse if cg.inline_on => {
            let b = matches!(op, Op::LoadTrue);
            let w0 = fb
                .ins()
                .iconst(types::I64, (cg.tags.bool_ as i64) | ((b as i64) << 8));
            let w1 = fb.ins().iconst(types::I64, 0);
            cg.vst.push(VVal { w0, w1, tag: Some(cg.tags.bool_), bit: None, truthy: Some(b) });
            false
        }
        Op::LoadSymbol(id) if cg.inline_on => {
            let w0 = fb
                .ins()
                .iconst(types::I64, (cg.tags.sym as i64) | ((id.0 as i64) << 32));
            let w1 = fb.ins().iconst(types::I64, 0);
            cg.vst.push(VVal { w0, w1, tag: Some(cg.tags.sym), bit: None, truthy: Some(true) });
            false
        }
        // ------------------------------------------------------------------
        // Self / locals (Locals::Stack read cache; write-through stores).
        // ------------------------------------------------------------------
        Op::LoadSelf if cg.inline_on => {
            let (sw0, sw1) = (cg.self_w0.expect("self regs"), cg.self_w1.expect("self regs"));
            let snap = cg.snapshot();
            let mut fail_b: Option<Block> = None;
            let tag = cg.tag_from_w0(fb, sw0);
            let triv = cg.mask_test(fb, cg.tags.trivial_mask, tag);
            let ok = fb.ins().icmp_imm(IntCC::NotEqual, triv, 0);
            cg.guard(fb, ok, &mut fail_b, &snap, i);
            cg.vst.push(VVal::raw(sw0, sw1));
            false
        }
        // LITE-BLOCK captured-outer slot access (`s < param_start`): the
        // slot lives in the canonical binding cell, not the native spill —
        // route through the cell helpers. The read pushes a CLONE onto the
        // real operand stack (an owned root; the vst is flushed first so
        // stack order is exact); the write pops the (flushed) top into the
        // cell. Neither can fail, raise, or GC-allocate; no SSA caching
        // (own-region only, the wave-3 discipline).
        Op::LoadLocal(s) if cg.lite_blk && s < cg.lite_ps => {
            cg.flush(fb);
            let blkid = cg.lite_blkid.expect("lite blkid");
            let slotc = fb.ins().iconst(types::I64, s as i64);
            fb.ins().call(cg.h.blk_outer_get, &[vm, blkid, slotc]);
            cg.invalidate_mem();
            false
        }
        Op::StoreLocal(s) if cg.lite_blk && s < cg.lite_ps => {
            cg.flush(fb);
            let blkid = cg.lite_blkid.expect("lite blkid");
            let slotc = fb.ins().iconst(types::I64, s as i64);
            fb.ins().call(cg.h.blk_outer_set, &[vm, blkid, slotc]);
            cg.invalidate_mem();
            // Outer slots are never SSA-cached, but clear defensively so a
            // future lowering can't observe a stale pair.
            cg.cache[s as usize] = None;
            false
        }
        Op::LoadLocal(s) if cg.cacheable && (s as usize) < cg.cache.len() => {
            if let Some((w0, w1)) = cg.cache[s as usize] {
                cg.vst.push(VVal::raw(w0, w1));
                return false;
            }
            let snap = cg.snapshot();
            let (addr, off) = cg.local_addr(fb, s);
            let w0 = fb.ins().load(types::I64, fl(), addr, off);
            let w1 = fb.ins().load(types::I64, fl(), addr, off + 8);
            let tag = cg.tag_from_w0(fb, w0);
            let triv = cg.mask_test(fb, cg.tags.trivial_mask, tag);
            let ok = fb.ins().icmp_imm(IntCC::NotEqual, triv, 0);
            let mut fail_b: Option<Block> = None;
            cg.guard(fb, ok, &mut fail_b, &snap, i);
            cg.cache[s as usize] = Some((w0, w1));
            cg.vst.push(VVal::raw(w0, w1));
            false
        }
        Op::StoreLocal(s) if cg.cacheable && (s as usize) < cg.cache.len() => {
            let snap = cg.snapshot();
            // Peek the value (virtual or real top) WITHOUT consuming, then
            // guard the OLD slot value's drop-freeness, then commit.
            let (addr, off) = cg.local_addr(fb, s);
            let old_w0 = fb.ins().load(types::I64, fl(), addr, off);
            let old_tag = cg.tag_from_w0(fb, old_w0);
            let triv = cg.mask_test(fb, cg.tags.trivial_mask, old_tag);
            let ok = fb.ins().icmp_imm(IntCC::NotEqual, triv, 0);
            let mut fail_b: Option<Block> = None;
            cg.guard(fb, ok, &mut fail_b, &snap, i);
            match cg.vst.pop() {
                Some(v) => {
                    fb.ins().store(fl(), v.w0, addr, off);
                    fb.ins().store(fl(), v.w1, addr, off + 8);
                    cg.cache[s as usize] = Some((v.w0, v.w1));
                }
                None => {
                    // Real move: 16-byte transfer off the stack top (any
                    // tag — ownership moves, no clone/drop needed).
                    let a = cg.stack_slot_addr(fb, 0);
                    let w0 = fb.ins().load(types::I64, fl(), a, 0);
                    let w1 = fb.ins().load(types::I64, fl(), a, 8);
                    if cg.lite {
                        // FRAME-LITE: a native local slot may never OWN a
                        // non-trivial value (the materialize ownership
                        // accounting relies on "non-trivial slot word ⟹
                        // the untouched arg borrow") — moving one in
                        // materializes instead. Combined with the old-value
                        // guard above, non-trivial words can only ever
                        // enter local slots via the entry arg binding.
                        let tag = cg.tag_from_w0(fb, w0);
                        let triv = cg.mask_test(fb, cg.tags.trivial_mask, tag);
                        let ok2 = fb.ins().icmp_imm(IntCC::NotEqual, triv, 0);
                        cg.guard(fb, ok2, &mut fail_b, &snap, i);
                    }
                    cg.consume_reals(fb, 1);
                    fb.ins().store(fl(), w0, addr, off);
                    fb.ins().store(fl(), w1, addr, off + 8);
                    cg.cache[s as usize] = None; // tag unknown: not cacheable
                }
            }
            false
        }
        Op::IncLocal(s) | Op::IncLocalNoPush(s) if cg.cacheable && (s as usize) < cg.cache.len() => {
            let snap = cg.snapshot();
            let mut fail_b: Option<Block> = None;
            let (addr, off) = cg.local_addr(fb, s);
            let (w0, w1) = match cg.cache[s as usize] {
                Some((w0, w1)) => (w0, w1),
                None => {
                    let w0 = fb.ins().load(types::I64, fl(), addr, off);
                    let w1 = fb.ins().load(types::I64, fl(), addr, off + 8);
                    (w0, w1)
                }
            };
            let tag = cg.tag_from_w0(fb, w0);
            let is_int = fb.ins().icmp_imm(IntCC::Equal, tag, cg.tags.int as i64);
            cg.guard(fb, is_int, &mut fail_b, &snap, i);
            // The interpreter's IncLocal uses wrapping_add — the slow path
            // (`+` re-dispatch) fires only for non-Int values. Mirror the
            // wrap exactly: plain iadd, no overflow bail.
            let nv = fb.ins().iadd_imm(w1, 1);
            fb.ins().store(fl(), nv, addr, off + 8);
            cg.cache[s as usize] = Some((w0, nv));
            if matches!(op, Op::IncLocal(_)) {
                let vv = cg.vv_int(fb, nv);
                cg.vst.push(vv);
            }
            false
        }
        // ------------------------------------------------------------------
        // Stack shuffles.
        // ------------------------------------------------------------------
        Op::Dup => {
            if let Some(&v) = cg.vst.last() {
                cg.vst.push(v);
            } else {
                cg.flush(fb);
                fb.ins().call(cg.h.dup, &[vm]);
                cg.invalidate_mem();
            }
            false
        }
        Op::Pop => {
            if cg.vst.pop().is_some() {
                // free
            } else if cg.inline_on {
                let a = cg.stack_slot_addr(fb, 0);
                let w0 = fb.ins().load(types::I64, fl(), a, 0);
                let tag = cg.tag_from_w0(fb, w0);
                let triv = cg.mask_test(fb, cg.tags.trivial_mask, tag);
                let fast_b = fb.create_block();
                let slow_b = fb.create_block();
                let merge = fb.create_block();
                fb.ins().brif(triv, fast_b, &[], slow_b, &[]);
                fb.switch_to_block(fast_b);
                let len = cg.stack_len(fb);
                let nl = fb.ins().iadd_imm(len, -1);
                cg.set_stack_len(fb, nl);
                fb.ins().jump(merge, &[]);
                fb.switch_to_block(slow_b);
                fb.ins().call(cg.h.pop, &[vm]);
                fb.ins().jump(merge, &[]);
                fb.switch_to_block(merge);
                cg.invalidate_mem();
            } else {
                fb.ins().call(cg.h.pop, &[vm]);
            }
            false
        }
        Op::Swap => {
            if cg.vst.len() >= 2 {
                let n = cg.vst.len();
                cg.vst.swap(n - 1, n - 2);
            } else {
                cg.flush(fb);
                fb.ins().call(cg.h.swap, &[vm]);
                cg.invalidate_mem();
            }
            false
        }
        // ------------------------------------------------------------------
        // Ivars.
        // ------------------------------------------------------------------
        Op::LoadIvar(sym, cid) if cg.inline_on => {
            let snap = cg.snapshot();
            let mut fail_b: Option<Block> = None;
            let sw0 = cg.self_w0.expect("self regs");
            let tag = cg.tag_from_w0(fb, sw0);
            let is_obj = fb.ins().icmp_imm(IntCC::Equal, tag, cg.tags.object as i64);
            cg.guard(fb, is_obj, &mut fail_b, &snap, i);
            // The helper may push (non-trivial ivar value) — flush first.
            // (Lite: the helper never pushes — a non-trivial value DECLINES
            // and the guard below materializes; the flush keeps the decline
            // snapshot canonical either way.)
            cg.flush(fb);
            let oid = fb.ins().ushr_imm(sw0, 32);
            let symc = fb.ins().iconst(types::I64, sym.0 as i64);
            let cidc = fb.ins().iconst(types::I64, cid as i64);
            let out = cg.scratch.expect("scratch slot");
            if cg.lite {
                let call = fb.ins().call(cg.h.lite_ivar_get, &[vm, oid, symc, out]);
                let ret = fb.inst_results(call)[0];
                cg.invalidate_mem();
                // ret==0 → non-trivial ivar value: decline (the helper had
                // no effect) → materialize; the interpreter re-runs the op.
                let post = cg.snapshot();
                let ok = fb.ins().icmp_imm(IntCC::NotEqual, ret, 0);
                let mut fail2: Option<Block> = None;
                cg.guard(fb, ok, &mut fail2, &post, i);
                let w0 = fb.ins().load(types::I64, fl(), out, 0);
                let w1 = fb.ins().load(types::I64, fl(), out, 8);
                cg.vst.push(VVal::raw(w0, w1));
                return false;
            }
            let call = fb.ins().call(cg.h.ivar_get, &[vm, oid, symc, cidc, out]);
            let ret = fb.inst_results(call)[0];
            cg.invalidate_mem();
            // ret==1 → value words in scratch (stay virtual); ret==0 → the
            // helper pushed the real value: the op is COMPLETE with
            // canonical state — rejoin via an (empty-prefix) resume from
            // the NEXT op.
            let virt_b = fb.create_block();
            let pushed_b = fb.create_block();
            fb.ins().brif(ret, virt_b, &[], pushed_b, &[]);
            let canonical = CgSnap {
                vst: Vec::new(),
                cache: cg.cache.clone(),
                sptr: None,
                slen: None,
                scap: None,
                aptr: None,
            };
            cg.fill_resume(fb, pushed_b, &canonical, i + 1);
            fb.switch_to_block(virt_b);
            let w0 = fb.ins().load(types::I64, fl(), out, 0);
            let w1 = fb.ins().load(types::I64, fl(), out, 8);
            cg.vst.push(VVal::raw(w0, w1));
            false
        }
        Op::StoreIvar(sym, cid) if cg.inline_on && !cg.vst.is_empty() => {
            let v = cg.vst.pop().expect("checked nonempty");
            cg.flush(fb);
            let symc = fb.ins().iconst(types::I64, sym.0 as i64);
            if cg.lite {
                // Register-passing frameless variant (self from the entry
                // regs); a frozen receiver DECLINES → materialize with the
                // stored value re-materialized on the operand stack, so the
                // interpreter re-runs the op (and raises the canonical
                // FrozenError against the real frame).
                let (sw0, sw1) = (
                    cg.self_w0.expect("self regs"),
                    cg.self_w1.expect("self regs"),
                );
                let snap = {
                    let mut s = cg.snapshot();
                    s.vst = vec![v];
                    s
                };
                let call = fb
                    .ins()
                    .call(cg.h.lite_ivar_set, &[vm, symc, v.w0, v.w1, sw0, sw1]);
                let ret = fb.inst_results(call)[0];
                cg.invalidate_mem();
                let ok = fb.ins().icmp_imm(IntCC::Equal, ret, 0);
                let mut fail_b: Option<Block> = None;
                cg.guard(fb, ok, &mut fail_b, &snap, i);
                return false;
            }
            let cidc = fb.ins().iconst(types::I64, cid as i64);
            let ipc = fb.ins().iconst(types::I64, i as i64);
            let call = fb
                .ins()
                .call(cg.h.ivar_set_v, &[vm, symc, cidc, v.w0, v.w1, ipc]);
            let st = fb.inst_results(call)[0];
            cg.check_status(fb, st);
            cg.invalidate_mem();
            false
        }
        // Lean stack-value serve (campaign P5a): the stored value is on
        // the REAL operand stack (call-fed stores, `!inline_on` bodies)
        // — previously the generic `t2_op` boundary (the AM census's
        // StoreIvar 55.7/iter row). The flush lands any remaining
        // virtuals first, so the helper's pop sees exactly the
        // interpreter's stack. StoreIvar never writes locals, so the
        // local read cache survives (unlike `emit_generic`'s
        // `clear_cache`). LITE keeps its materialize-bail via the
        // default arm (a lite body has no frame for the helper's ip
        // stamp / trap discipline).
        Op::StoreIvar(sym, cid) if !cg.lite => {
            cg.flush(fb);
            let symc = fb.ins().iconst(types::I64, sym.0 as i64);
            let cidc = fb.ins().iconst(types::I64, cid as i64);
            let ipc = fb.ins().iconst(types::I64, i as i64);
            let call = fb.ins().call(cg.h.store_ivar, &[vm, symc, cidc, ipc]);
            let st = fb.inst_results(call)[0];
            cg.check_status(fb, st);
            cg.invalidate_mem();
            false
        }
        // Lean `Op::InterpToS` serve (campaign P6b): the step arm's
        // interpolation `to_s` — String passthrough + Symbol/Integer
        // primitive fast serve inline, declining to the full
        // `do_call(:to_s)` only for a user `to_s` (or a non-fast
        // primitive / reopened|refined shape) — instead of the generic
        // `t2_op` boundary (the AM census's InterpToS row). The flush
        // lands any virtuals so the helper's `stack.last()` sees the
        // interpreter's stack. InterpToS never writes locals and the
        // declined `to_s` runs in a fresh callee frame that can't touch
        // this frame's `Locals::Stack` slots, so (like the call arms /
        // stack StoreIvar) the local read cache survives; `invalidate_mem`
        // covers a user `to_s`'s ivar side effects. LITE bails via the
        // default arm (no frame for the decline's ip stamp / t2_finish).
        Op::InterpToS(cid) if !cg.lite => {
            cg.flush(fb);
            let cidc = fb.ins().iconst(types::I64, cid as i64);
            let ipc = fb.ins().iconst(types::I64, i as i64);
            let call = fb.ins().call(cg.h.interp_to_s, &[vm, cidc, ipc]);
            let st = fb.inst_results(call)[0];
            cg.check_status(fb, st);
            cg.invalidate_mem();
            false
        }
        // ------------------------------------------------------------------
        // Small-Int arithmetic / comparisons + Sym equality.
        // ------------------------------------------------------------------
        Op::BinOp(kind) if cg.inline_on && binop_inlineable(kind) => {
            emit_binop(cg, fb, i, kind, None)
        }
        Op::BinOpInt(kind, rhs) if cg.inline_on && binop_inlineable(kind) => {
            emit_binop(cg, fb, i, kind, Some(BinRhs::ConstInt(rhs)))
        }
        Op::BinOpLocalLocal(kind, a_slot, b_slot)
            if cg.cacheable
                && binop_inlineable(kind)
                && (a_slot as usize) < cg.cache.len()
                && (b_slot as usize) < cg.cache.len() =>
        {
            emit_binop(cg, fb, i, kind, Some(BinRhs::Locals(a_slot, b_slot)))
        }
        // ------------------------------------------------------------------
        // CaseEqLit (lowered `when <literal>`).
        // ------------------------------------------------------------------
        Op::CaseEqLit(lit, _cid) if cg.inline_on && case_lit_kind(&lit).is_some() => {
            let (kind, payload) = case_lit_kind(&lit).expect("checked");
            let kc = fb.ins().iconst(types::I64, kind);
            let pc = fb.ins().iconst(types::I64, payload);
            // Pop the predicate FIRST, flush the rest, and only then take
            // the decline snapshot: the rest is already materialized, so
            // the slow edge must re-materialize ONLY the predicate.
            let arg_opt = cg.vst.pop();
            cg.flush(fb);
            let snap = {
                let mut s = cg.snapshot();
                if let Some(a) = arg_opt {
                    s.vst = vec![a];
                }
                s
            };
            let ret = if let Some(arg) = arg_opt {
                let call = fb
                    .ins()
                    .call(cg.h.case_eq_v, &[vm, kc, pc, arg.w0, arg.w1]);
                fb.inst_results(call)[0]
            } else {
                let call = fb.ins().call(cg.h.case_eq_s, &[vm, kc, pc]);
                fb.inst_results(call)[0]
            };
            cg.invalidate_mem();
            // ret 0/1 = answer; 2 = decline → re-run the op interpreted
            // (the slow edge re-materializes the register-borne predicate).
            let ok = fb.ins().icmp_imm(IntCC::UnsignedLessThan, ret, 2);
            let mut fail_b: Option<Block> = None;
            cg.guard(fb, ok, &mut fail_b, &snap, i);
            let v = cg.vv_bool_bit(fb, ret);
            cg.vst.push(v);
            false
        }
        // ------------------------------------------------------------------
        // Wave-1/2 helper fallbacks for the simple ops (Shared frames,
        // noinline mode, probe failure).
        // ------------------------------------------------------------------
        Op::LoadConstInt(v) => {
            let c = fb.ins().iconst(types::I64, v);
            fb.ins().call(cg.h.push_int, &[vm, c]);
            cg.invalidate_mem();
            false
        }
        Op::LoadNil => {
            fb.ins().call(cg.h.push_nil, &[vm]);
            cg.invalidate_mem();
            false
        }
        Op::LoadTrue | Op::LoadFalse => {
            let c = fb
                .ins()
                .iconst(types::I64, matches!(op, Op::LoadTrue) as i64);
            fb.ins().call(cg.h.push_bool, &[vm, c]);
            cg.invalidate_mem();
            false
        }
        Op::LoadSymbol(id) => {
            let c = fb.ins().iconst(types::I64, id.0 as i64);
            fb.ins().call(cg.h.push_sym, &[vm, c]);
            cg.invalidate_mem();
            false
        }
        Op::LoadSelf => {
            cg.flush(fb);
            fb.ins().call(cg.h.load_self, &[vm]);
            cg.invalidate_mem();
            false
        }
        Op::LoadLocal(s) => {
            cg.flush(fb);
            let c = fb.ins().iconst(types::I64, s as i64);
            fb.ins().call(cg.h.load_local, &[vm, c]);
            cg.invalidate_mem();
            false
        }
        Op::StoreLocal(s) => {
            cg.flush(fb);
            let c = fb.ins().iconst(types::I64, s as i64);
            fb.ins().call(cg.h.store_local, &[vm, c]);
            cg.invalidate_mem();
            false
        }
        Op::LoadIvar(sym, cid) => {
            cg.flush(fb);
            let c = fb.ins().iconst(types::I64, sym.0 as i64);
            let cidc = fb.ins().iconst(types::I64, cid as i64);
            fb.ins().call(cg.h.load_ivar, &[vm, c, cidc]);
            cg.invalidate_mem();
            false
        }
        // Cvar family (campaign P4): the AM census's largest generic-op
        // family (LoadCvar+StoreCvar 255/iter through t2_op). Lean
        // helpers ride the interpreter's own per-site owner cache and
        // skip the generic boundary (op fetch + step match + t2_finish
        // — neither op can trap, push a frame, or GC-allocate).
        Op::LoadCvar(sym, cid) => {
            cg.flush(fb);
            let c = fb.ins().iconst(types::I64, sym.0 as i64);
            let cidc = fb.ins().iconst(types::I64, cid as i64);
            fb.ins().call(cg.h.load_cvar, &[vm, c, cidc]);
            cg.invalidate_mem();
            false
        }
        Op::StoreCvar(sym, cid) => {
            cg.flush(fb);
            let c = fb.ins().iconst(types::I64, sym.0 as i64);
            let cidc = fb.ins().iconst(types::I64, cid as i64);
            fb.ins().call(cg.h.store_cvar, &[vm, c, cidc]);
            cg.invalidate_mem();
            false
        }
        // --- everything else: full interpreter semantics ---
        _ => cg.emit_generic(fb, i),
    }
}

/// The BinOp kinds the inline lowering handles (Div/Mod keep the
/// interpreter's floor-division + ZeroDivisionError semantics via the
/// generic helper).
fn binop_inlineable(kind: BinOpKind) -> bool {
    !matches!(kind, BinOpKind::Div | BinOpKind::Mod)
}

enum BinRhs {
    /// `Op::BinOpInt` — the rhs is a baked Int immediate.
    ConstInt(i64),
    /// `Op::BinOpLocalLocal` — both operands come from local slots.
    Locals(u16, u16),
}

fn case_lit_kind(lit: &CaseLit) -> Option<(i64, i64)> {
    Some(match lit {
        CaseLit::Sym(s) => (0, s.0 as i64),
        CaseLit::Int(v) => (1, *v),
        CaseLit::True => (2, 1),
        CaseLit::False => (2, 0),
        CaseLit::Nil => (3, 0),
        CaseLit::Float(f) => (4, f.to_bits() as i64),
        CaseLit::Str(_) => return None, // frozen-literal + alloc semantics → generic
    })
}

/// Inline lowering for the BinOp family. Int×Int arithmetic (wrapping →
/// overflow bails to the interpreter's BigInt promotion), Int×Int
/// comparisons, and same-tag Int/Sym (in)equality — exactly the pairs the
/// interpreter serves from its own fast arms BEFORE any user-defined method
/// could be consulted (mixed-tag `==` falls to `do_call`, so it resumes).
/// Returns false (never terminates the chain).
fn emit_binop(
    cg: &mut Cg,
    fb: &mut FunctionBuilder,
    i: usize,
    kind: BinOpKind,
    rhs_kind: Option<BinRhs>,
) -> bool {
    let snap = cg.snapshot();
    let mut fail_b: Option<Block> = None;
    let int_tag = cg.tags.int;

    // Acquire operands.
    let (lhs, rhs, n_real, cache_slots) = match rhs_kind {
        None => {
            let (l, r, n) = cg.take2(fb);
            (l, r, n, None)
        }
        Some(BinRhs::ConstInt(v)) => {
            let (l, n) = match cg.vst.pop() {
                Some(vv) => (Operand { w0: vv.w0, w1: vv.w1, tag: vv.tag }, 0i64),
                None => {
                    let a = cg.stack_slot_addr(fb, 0);
                    let w0 = fb.ins().load(types::I64, fl(), a, 0);
                    let w1 = fb.ins().load(types::I64, fl(), a, 8);
                    (Operand { w0, w1, tag: None }, 1i64)
                }
            };
            let w0 = fb.ins().iconst(types::I64, int_tag as i64);
            let w1 = fb.ins().iconst(types::I64, v);
            (l, Operand { w0, w1, tag: Some(int_tag) }, n, None)
        }
        Some(BinRhs::Locals(a_slot, b_slot)) => {
            let read = |cg: &mut Cg, fb: &mut FunctionBuilder, s: u16| {
                // LITE-BLOCK outer operand: an effect-free register read
                // through the canonical cell (never cached — the cell is
                // not the spill, and outer StoreLocal wouldn't refresh it).
                if cg.lite_blk && s < cg.lite_ps {
                    let blkid = cg.lite_blkid.expect("lite blkid");
                    let out = cg.scratch.expect("lite scratch");
                    let slotc = fb.ins().iconst(types::I64, s as i64);
                    fb.ins().call(cg.h.blk_outer_read, &[cg.vm, blkid, slotc, out]);
                    let w0 = fb.ins().load(types::I64, fl(), out, 0);
                    let w1 = fb.ins().load(types::I64, fl(), out, 8);
                    return (Operand { w0, w1, tag: None }, false);
                }
                if let Some((w0, w1)) = cg.cache[s as usize] {
                    (Operand { w0, w1, tag: None }, false)
                } else {
                    let (addr, off) = cg.local_addr(fb, s);
                    let w0 = fb.ins().load(types::I64, fl(), addr, off);
                    let w1 = fb.ins().load(types::I64, fl(), addr, off + 8);
                    (Operand { w0, w1, tag: None }, true)
                }
            };
            let (l, l_fresh) = read(cg, fb, a_slot);
            let (r, r_fresh) = read(cg, fb, b_slot);
            (l, r, 0i64, Some((a_slot, b_slot, l_fresh, r_fresh)))
        }
    };

    let is_eqne = matches!(kind, BinOpKind::Eq | BinOpKind::Ne);
    if !is_eqne {
        // Arithmetic / ordered comparison: both must be Int.
        if lhs.tag.is_some_and(|t| t != int_tag) || rhs.tag.is_some_and(|t| t != int_tag) {
            // Statically not Int×Int: the generic arm owns it (completes
            // the op; the chain continues canonically). In lite mode this
            // TERMINATES the chain (materialize + bail).
            cg.restore(snap);
            return cg.emit_generic(fb, i);
        }
        if !cg.guard_tag(fb, &lhs, int_tag, &mut fail_b, &snap, i) {
            unreachable!("static tag mismatch handled above");
        }
        if !cg.guard_tag(fb, &rhs, int_tag, &mut fail_b, &snap, i) {
            unreachable!("static tag mismatch handled above");
        }
        // Both-Int certified: cache freshly-loaded local operands
        // (`l_fresh`/`r_fresh` are false for lite-block outer operands —
        // their register reads never enter the cache).
        if let Some((a_slot, b_slot, l_fresh, r_fresh)) = cache_slots {
            if l_fresh {
                cg.cache[a_slot as usize] = Some((lhs.w0, lhs.w1));
            }
            if r_fresh {
                cg.cache[b_slot as usize] = Some((rhs.w0, rhs.w1));
            }
        }
        match kind {
            BinOpKind::Add | BinOpKind::Sub | BinOpKind::Mul => {
                let (res, of) = match kind {
                    BinOpKind::Add => fb.ins().sadd_overflow(lhs.w1, rhs.w1),
                    BinOpKind::Sub => fb.ins().ssub_overflow(lhs.w1, rhs.w1),
                    _ => fb.ins().smul_overflow(lhs.w1, rhs.w1),
                };
                // Overflow → the interpreter's BigInt promotion (bignum) or
                // wrapping fallback (no-bignum) — either way, re-run the op.
                let no_of = fb.ins().bxor_imm(of, 1);
                cg.guard(fb, no_of, &mut fail_b, &snap, i);
                cg.consume_reals(fb, n_real);
                let vv = cg.vv_int(fb, res);
                cg.vst.push(vv);
            }
            BinOpKind::Lt | BinOpKind::Le | BinOpKind::Gt | BinOpKind::Ge => {
                let cc = match kind {
                    BinOpKind::Lt => IntCC::SignedLessThan,
                    BinOpKind::Le => IntCC::SignedLessThanOrEqual,
                    BinOpKind::Gt => IntCC::SignedGreaterThan,
                    _ => IntCC::SignedGreaterThanOrEqual,
                };
                let bit = fb.ins().icmp(cc, lhs.w1, rhs.w1);
                cg.consume_reals(fb, n_real);
                let vv = cg.vv_bool_bit(fb, bit);
                cg.vst.push(vv);
            }
            _ => unreachable!("Div/Mod filtered by binop_inlineable"),
        }
        return false;
    }

    // Eq / Ne: same-tag Int×Int or Sym×Sym only (the interpreter serves
    // those from `apply_int` / the `(Sym, "==", Sym)` primitive arm before
    // any user table; every other pairing goes through `do_call`).
    let sym_tag = cg.tags.sym;
    match (lhs.tag, rhs.tag) {
        (Some(a), Some(b)) if a == b && (a == int_tag || a == sym_tag) => {
            let (ka, kb) = if a == int_tag {
                (lhs.w1, rhs.w1)
            } else {
                let ka = fb.ins().ushr_imm(lhs.w0, 32);
                let kb = fb.ins().ushr_imm(rhs.w0, 32);
                (ka, kb)
            };
            let cc = if matches!(kind, BinOpKind::Eq) { IntCC::Equal } else { IntCC::NotEqual };
            let bit = fb.ins().icmp(cc, ka, kb);
            cg.consume_reals(fb, n_real);
            let vv = cg.vv_bool_bit(fb, bit);
            cg.vst.push(vv);
        }
        (Some(_), Some(_)) => {
            // Statically mixed / unsupported: generic arm (terminates the
            // chain in lite mode — materialize + bail).
            cg.restore(snap);
            return cg.emit_generic(fb, i);
        }
        _ => {
            // Runtime same-tag check: (Int,Int) or (Sym,Sym).
            let tl = cg.tag_from_w0(fb, lhs.w0);
            let tr = cg.tag_from_w0(fb, rhs.w0);
            let li = fb.ins().icmp_imm(IntCC::Equal, tl, int_tag as i64);
            let ri = fb.ins().icmp_imm(IntCC::Equal, tr, int_tag as i64);
            let both_int = fb.ins().band(li, ri);
            let ls = fb.ins().icmp_imm(IntCC::Equal, tl, sym_tag as i64);
            let rs = fb.ins().icmp_imm(IntCC::Equal, tr, sym_tag as i64);
            let both_sym = fb.ins().band(ls, rs);
            let ok = fb.ins().bor(both_int, both_sym);
            cg.guard(fb, ok, &mut fail_b, &snap, i);
            // key = Int ? w1 : SymId (w0 high 32).
            let l_sym_key = fb.ins().ushr_imm(lhs.w0, 32);
            let r_sym_key = fb.ins().ushr_imm(rhs.w0, 32);
            let ka = fb.ins().select(both_int, lhs.w1, l_sym_key);
            let kb = fb.ins().select(both_int, rhs.w1, r_sym_key);
            let cc = if matches!(kind, BinOpKind::Eq) { IntCC::Equal } else { IntCC::NotEqual };
            let bit = fb.ins().icmp(cc, ka, kb);
            if let Some((a_slot, b_slot, l_fresh, r_fresh)) = cache_slots {
                // Tag certified trivial (Int/Sym) — cacheable.
                if l_fresh {
                    cg.cache[a_slot as usize] = Some((lhs.w0, lhs.w1));
                }
                if r_fresh {
                    cg.cache[b_slot as usize] = Some((rhs.w0, rhs.w1));
                }
            }
            cg.consume_reals(fb, n_real);
            let vv = cg.vv_bool_bit(fb, bit);
            cg.vst.push(vv);
        }
    }
    false
}

/// Compile `proto`'s body to a tier-2 native function. Returns `None` when
/// the body is not admitted (or codegen fails) — the caller records the
/// verdict and keeps interpreting. Mode controls (via `ctx`):
/// `nocall` reproduces the wave-1 tier, `noinline` the wave-2 tier; a
/// failed `Vec` layout probe also disables the inline lowering (helper
/// emission is the universal fallback, never a miscompile).
pub(crate) fn compile_tier2(proto: &Proto, proto_idx: usize, ctx: &T2Ctx) -> Option<T2Proto> {
    let dbg = std::env::var_os("RUBYRS_JIT_TIER2_DEBUG").is_some();
    if let Err(why) = t2_admit(proto) {
        if dbg {
            eprintln!("t2 decline {:<28} ({} ops): {}", proto.name, proto.code.len(), why);
        }
        return None;
    }
    if dbg {
        eprintln!("t2 admit   {:<28} ({} ops)", proto.name, proto.code.len());
    }
    let code = &proto.code;
    let n = code.len();
    // The inline lowering needs the probed Vec layout; `nocall` (wave-1
    // repro) and `noinline` (wave-2 repro) both disable it.
    let inline_on = !ctx.nocall && !ctx.noinline && vec_layout().is_some();
    // Local-slot SSA caching: only method/toplevel-style protos whose
    // locals are `Locals::Stack` (escape-analysed: no CreateBlock in the
    // body, and not a block proto — block frames are always `Shared` and
    // capture-routed, so their slots stay helper-routed). The entry check
    // double-guards the representation at runtime (Shared → immediate BAIL
    // at ip 0, i.e. the interpreter runs the body).
    let cacheable =
        inline_on && !proto.creates_block && proto.block_body_local_start == u16::MAX;

    // Leader scan: block boundaries at every jump target + fallthrough.
    let mut leader = vec![false; n];
    leader[0] = true;
    for (i, op) in code.iter().enumerate() {
        match op {
            Op::Jump(off) | Op::BreakLoop(off) | Op::NextLoop(off) => {
                leader[jump_target(i, *off)] = true;
                if i + 1 < n {
                    leader[i + 1] = true;
                }
            }
            Op::JumpIfFalse(off) | Op::JumpIfArgGiven(_, off) | Op::JumpIfKwArgGiven(_, off) => {
                leader[jump_target(i, *off)] = true;
                leader[i + 1] = true;
            }
            Op::Return if i + 1 < n => {
                leader[i + 1] = true;
            }
            _ => {}
        }
    }

    let mut builder = JITBuilder::new(cranelift_module::default_libcall_names()).ok()?;
    builder.symbol("t2_op", t2_op as *const u8);
    builder.symbol("t2_resume", t2_resume as *const u8);
    builder.symbol("t2_entry_info", t2_entry_info as *const u8);
    builder.symbol("t2_stack_reserve", t2_stack_reserve as *const u8);
    builder.symbol("t2_poll", t2_poll as *const u8);
    builder.symbol("t2_ivar_get", t2_ivar_get as *const u8);
    builder.symbol("t2_ivar_set_v", t2_ivar_set_v as *const u8);
    builder.symbol("t2_case_eq_v", t2_case_eq_v as *const u8);
    builder.symbol("t2_case_eq_s", t2_case_eq_s as *const u8);
    builder.symbol("t2_return_v", t2_return_v as *const u8);
    builder.symbol("t2_pop_truthy", t2_pop_truthy as *const u8);
    builder.symbol("t2_arg_given", t2_arg_given as *const u8);
    builder.symbol("t2_kwarg_given", t2_kwarg_given as *const u8);
    builder.symbol("t2_push_int", t2_push_int as *const u8);
    builder.symbol("t2_push_nil", t2_push_nil as *const u8);
    builder.symbol("t2_push_bool", t2_push_bool as *const u8);
    builder.symbol("t2_push_sym", t2_push_sym as *const u8);
    builder.symbol("t2_load_self", t2_load_self as *const u8);
    builder.symbol("t2_load_local", t2_load_local as *const u8);
    builder.symbol("t2_store_local", t2_store_local as *const u8);
    builder.symbol("t2_load_ivar", t2_load_ivar as *const u8);
    builder.symbol("t2_load_cvar", t2_load_cvar as *const u8);
    builder.symbol("t2_store_cvar", t2_store_cvar as *const u8);
    builder.symbol("t2_store_ivar", t2_store_ivar as *const u8);
    builder.symbol("t2_interp_to_s", t2_interp_to_s as *const u8);
    builder.symbol("t2_super", t2_super as *const u8);
    builder.symbol("t2_dup", t2_dup as *const u8);
    builder.symbol("t2_pop", t2_pop as *const u8);
    builder.symbol("t2_swap", t2_swap as *const u8);
    builder.symbol("t2_call", t2_call as *const u8);
    builder.symbol("t2_call_norecv", t2_call_norecv as *const u8);
    builder.symbol("t2_call_local", t2_call_local as *const u8);
    builder.symbol("t2_return", t2_return as *const u8);
    builder.symbol("t2_call_block", t2_call_block as *const u8);
    builder.symbol("t2_call_norecv_block", t2_call_norecv_block as *const u8);
    builder.symbol("t2_yield", t2_yield as *const u8);
    builder.symbol("t2_lite_materialize", t2_lite_materialize as *const u8);
    builder.symbol("t2_lite_materialize_blk", t2_lite_materialize_blk as *const u8);
    builder.symbol("t2_lite_blk_outer_get", t2_lite_blk_outer_get as *const u8);
    builder.symbol("t2_lite_blk_outer_read", t2_lite_blk_outer_read as *const u8);
    builder.symbol("t2_lite_blk_outer_set", t2_lite_blk_outer_set as *const u8);
    builder.symbol("t2_lite_return_v", t2_lite_return_v as *const u8);
    builder.symbol("t2_lite_return_s", t2_lite_return_s as *const u8);
    builder.symbol("t2_lite_ivar_get", t2_lite_ivar_get as *const u8);
    builder.symbol("t2_lite_ivar_set", t2_lite_ivar_set as *const u8);
    builder.symbol("t2_lite_call_ex", t2_lite_call_ex as *const u8);
    builder.symbol("t2_lite_call_ns", t2_lite_call_ns as *const u8);
    builder.symbol("t2_lite_call_local", t2_lite_call_local as *const u8);
    builder.symbol("t2_lite_const_chain", t2_lite_const_chain as *const u8);
    builder.symbol("t2_lite_const_flat", t2_lite_const_flat as *const u8);
    builder.symbol("t2_const_flat", t2_const_flat as *const u8);
    builder.symbol("t2_const_chain", t2_const_chain as *const u8);
    builder.symbol("t2_push_const_str", t2_push_const_str as *const u8);
    let mut module = JITModule::new(builder);
    let ptr_ty = module.target_config().pointer_type();

    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(ptr_ty)); // vm
    sig.returns.push(AbiParam::new(types::I64)); // status
    let mut clctx = module.make_context();
    clctx.func.signature = sig.clone();
    let fid = module.declare_function("t2body", Linkage::Export, &sig).ok()?;
    // Wave-4 frame-lite sibling function (same module): admitted bodies get
    // a second, frameless entry `(vm, self_w0, self_w1, n_pop) -> status`.
    // Block protos get the LITE-BLOCK sibling instead
    // (`(vm, self_w0, self_w1, block_id) -> status`, ADR 0037 block-frame
    // residue) — a proto is one or the other.
    let lite_mode = if inline_on && !ctx.nolite {
        if proto.block_shape.is_some() {
            match t2_admit_lite_block(proto, ctx) {
                Ok((ps, np, rest)) => {
                    if dbg {
                        eprintln!(
                            "t2-liteblk admit   {:<28} ({} ops, ps {} np {}{})",
                            proto.name, n, ps, np, if rest { " rest" } else { "" }
                        );
                    }
                    LiteMode::Block(ps, np, rest)
                }
                Err(why) => {
                    if dbg {
                        eprintln!("t2-liteblk decline {:<28} ({} ops): {}", proto.name, n, why);
                    }
                    LiteMode::Off
                }
            }
        } else {
            match t2_admit_lite(proto, ctx) {
                Ok(a) => {
                    if dbg {
                        eprintln!("t2-lite admit   {:<28} ({} ops, argc {})", proto.name, n, a);
                    }
                    LiteMode::Method(a)
                }
                Err(why) => {
                    if dbg {
                        eprintln!("t2-lite decline {:<28} ({} ops): {}", proto.name, n, why);
                    }
                    LiteMode::Off
                }
            }
        }
    } else {
        LiteMode::Off
    };
    let mut lite_sig = module.make_signature();
    lite_sig.params.push(AbiParam::new(ptr_ty)); // vm
    lite_sig.params.push(AbiParam::new(types::I64)); // self_w0
    lite_sig.params.push(AbiParam::new(types::I64)); // self_w1
    lite_sig.params.push(AbiParam::new(types::I64)); // n_pop
    lite_sig.returns.push(AbiParam::new(types::I64)); // status
    let lite_fid = if !matches!(lite_mode, LiteMode::Off) {
        Some(module.declare_function("t2lite", Linkage::Export, &lite_sig).ok()?)
    } else {
        None
    };

    // Helper-signature builder: params are `vm` + a shape string of
    // 'p' (pointer) / 'i' (i64) chars; `ret` adds an i64 return.
    let decl = |module: &mut JITModule, name: &str, shape: &str, ret: bool| {
        let mut s = module.make_signature();
        s.params.push(AbiParam::new(ptr_ty));
        for ch in shape.chars() {
            s.params.push(AbiParam::new(if ch == 'p' { ptr_ty } else { types::I64 }));
        }
        if ret {
            s.returns.push(AbiParam::new(types::I64));
        }
        module.declare_function(name, Linkage::Import, &s).ok()
    };

    let f_op = decl(&mut module, "t2_op", "pii", true)?;
    let f_resume = decl(&mut module, "t2_resume", "iii", true)?;
    let f_entry_info = decl(&mut module, "t2_entry_info", "p", false)?;
    let f_reserve = decl(&mut module, "t2_stack_reserve", "i", false)?;
    let f_poll = decl(&mut module, "t2_poll", "i", true)?;
    let f_ivar_get = decl(&mut module, "t2_ivar_get", "iiip", true)?;
    let f_ivar_set_v = decl(&mut module, "t2_ivar_set_v", "iiiii", true)?;
    let f_case_eq_v = decl(&mut module, "t2_case_eq_v", "iiii", true)?;
    let f_case_eq_s = decl(&mut module, "t2_case_eq_s", "ii", true)?;
    let f_return_v = decl(&mut module, "t2_return_v", "iiii", true)?;
    let f_pop_truthy = decl(&mut module, "t2_pop_truthy", "", true)?;
    let f_arg_given = decl(&mut module, "t2_arg_given", "i", true)?;
    let f_kwarg_given = decl(&mut module, "t2_kwarg_given", "i", true)?;
    let f_push_int = decl(&mut module, "t2_push_int", "i", false)?;
    let f_push_nil = decl(&mut module, "t2_push_nil", "", false)?;
    let f_push_bool = decl(&mut module, "t2_push_bool", "i", false)?;
    let f_push_sym = decl(&mut module, "t2_push_sym", "i", false)?;
    let f_load_self = decl(&mut module, "t2_load_self", "", false)?;
    let f_load_local = decl(&mut module, "t2_load_local", "i", false)?;
    let f_store_local = decl(&mut module, "t2_store_local", "i", false)?;
    let f_load_ivar = decl(&mut module, "t2_load_ivar", "ii", false)?;
    let f_load_cvar = decl(&mut module, "t2_load_cvar", "ii", false)?;
    let f_store_cvar = decl(&mut module, "t2_store_cvar", "ii", false)?;
    let f_store_ivar = decl(&mut module, "t2_store_ivar", "iii", true)?;
    let f_interp_to_s = decl(&mut module, "t2_interp_to_s", "ii", true)?;
    let f_super = decl(&mut module, "t2_super", "iiii", true)?;
    let f_dup = decl(&mut module, "t2_dup", "", false)?;
    let f_pop = decl(&mut module, "t2_pop", "", false)?;
    let f_swap = decl(&mut module, "t2_swap", "", false)?;
    let f_call = decl(&mut module, "t2_call", "iiii", true)?;
    let f_call_norecv = decl(&mut module, "t2_call_norecv", "iiii", true)?;
    let f_call_local = decl(&mut module, "t2_call_local", "iiii", true)?;
    let f_return = decl(&mut module, "t2_return", "ii", true)?;
    let f_call_block = decl(&mut module, "t2_call_block", "iiii", true)?;
    let f_call_norecv_block = decl(&mut module, "t2_call_norecv_block", "iiii", true)?;
    let f_yield = decl(&mut module, "t2_yield", "ii", true)?;
    let f_lite_mat = decl(&mut module, "t2_lite_materialize", "iiiiiipii", false)?;
    let f_lite_mat_blk = decl(&mut module, "t2_lite_materialize_blk", "iiiiiipii", false)?;
    let f_blk_outer_get = decl(&mut module, "t2_lite_blk_outer_get", "ii", true)?;
    let f_blk_outer_read = decl(&mut module, "t2_lite_blk_outer_read", "iip", false)?;
    let f_blk_outer_set = decl(&mut module, "t2_lite_blk_outer_set", "ii", false)?;
    let f_lite_ret_v = decl(&mut module, "t2_lite_return_v", "iii", true)?;
    let f_lite_ret_s = decl(&mut module, "t2_lite_return_s", "i", true)?;
    let f_lite_ivar_get = decl(&mut module, "t2_lite_ivar_get", "iip", true)?;
    let f_lite_ivar_set = decl(&mut module, "t2_lite_ivar_set", "iiiii", true)?;
    let f_lite_call_ex = decl(&mut module, "t2_lite_call_ex", "piiiii", true)?;
    let f_lite_call_ns = decl(&mut module, "t2_lite_call_ns", "piiiii", true)?;
    let f_lite_call_local = decl(&mut module, "t2_lite_call_local", "piiiii", true)?;
    let f_lite_const = decl(&mut module, "t2_lite_const_chain", "piii", true)?;
    let f_lite_const_flat = decl(&mut module, "t2_lite_const_flat", "piii", true)?;
    let f_const_flat = decl(&mut module, "t2_const_flat", "ii", true)?;
    let f_const_chain = decl(&mut module, "t2_const_chain", "iii", true)?;
    let f_push_const_str = decl(&mut module, "t2_push_const_str", "ii", false)?;

    let make_refs = |module: &mut JITModule,
                     func: &mut cranelift_codegen::ir::Function|
     -> HelperRefs {
        HelperRefs {
            op: module.declare_func_in_func(f_op, func),
            resume: module.declare_func_in_func(f_resume, func),
            entry_info: module.declare_func_in_func(f_entry_info, func),
            stack_reserve: module.declare_func_in_func(f_reserve, func),
            poll: module.declare_func_in_func(f_poll, func),
            ivar_get: module.declare_func_in_func(f_ivar_get, func),
            ivar_set_v: module.declare_func_in_func(f_ivar_set_v, func),
            case_eq_v: module.declare_func_in_func(f_case_eq_v, func),
            case_eq_s: module.declare_func_in_func(f_case_eq_s, func),
            return_v: module.declare_func_in_func(f_return_v, func),
            pop_truthy: module.declare_func_in_func(f_pop_truthy, func),
            arg_given: module.declare_func_in_func(f_arg_given, func),
            kwarg_given: module.declare_func_in_func(f_kwarg_given, func),
            push_int: module.declare_func_in_func(f_push_int, func),
            push_nil: module.declare_func_in_func(f_push_nil, func),
            push_bool: module.declare_func_in_func(f_push_bool, func),
            push_sym: module.declare_func_in_func(f_push_sym, func),
            load_self: module.declare_func_in_func(f_load_self, func),
            load_local: module.declare_func_in_func(f_load_local, func),
            store_local: module.declare_func_in_func(f_store_local, func),
            load_ivar: module.declare_func_in_func(f_load_ivar, func),
            load_cvar: module.declare_func_in_func(f_load_cvar, func),
            store_cvar: module.declare_func_in_func(f_store_cvar, func),
            store_ivar: module.declare_func_in_func(f_store_ivar, func),
            interp_to_s: module.declare_func_in_func(f_interp_to_s, func),
            super_: module.declare_func_in_func(f_super, func),
            dup: module.declare_func_in_func(f_dup, func),
            pop: module.declare_func_in_func(f_pop, func),
            swap: module.declare_func_in_func(f_swap, func),
            call: module.declare_func_in_func(f_call, func),
            call_norecv: module.declare_func_in_func(f_call_norecv, func),
            call_local: module.declare_func_in_func(f_call_local, func),
            ret: module.declare_func_in_func(f_return, func),
            call_block: module.declare_func_in_func(f_call_block, func),
            call_norecv_block: module.declare_func_in_func(f_call_norecv_block, func),
            yield_: module.declare_func_in_func(f_yield, func),
            lite_mat: module.declare_func_in_func(f_lite_mat, func),
            lite_mat_blk: module.declare_func_in_func(f_lite_mat_blk, func),
            blk_outer_get: module.declare_func_in_func(f_blk_outer_get, func),
            blk_outer_read: module.declare_func_in_func(f_blk_outer_read, func),
            blk_outer_set: module.declare_func_in_func(f_blk_outer_set, func),
            lite_ret_v: module.declare_func_in_func(f_lite_ret_v, func),
            lite_ret_s: module.declare_func_in_func(f_lite_ret_s, func),
            lite_ivar_get: module.declare_func_in_func(f_lite_ivar_get, func),
            lite_ivar_set: module.declare_func_in_func(f_lite_ivar_set, func),
            lite_call_ex: module.declare_func_in_func(f_lite_call_ex, func),
            lite_call_ns: module.declare_func_in_func(f_lite_call_ns, func),
            lite_call_local: module.declare_func_in_func(f_lite_call_local, func),
            lite_const: module.declare_func_in_func(f_lite_const, func),
            lite_const_flat: module.declare_func_in_func(f_lite_const_flat, func),
            const_flat: module.declare_func_in_func(f_const_flat, func),
            const_chain: module.declare_func_in_func(f_const_chain, func),
            push_const_str: module.declare_func_in_func(f_push_const_str, func),
        }
    };

    let mut fbctx = FunctionBuilderContext::new();
    let emitted = {
        let mut fb = FunctionBuilder::new(&mut clctx.func, &mut fbctx);
        let h = make_refs(&mut module, fb.func);
        let ok = emit_body(
            &mut fb, &h, proto, proto_idx, code, n, &leader, ptr_ty, ctx, inline_on, cacheable,
            LiteMode::Off,
        );
        if ok {
            fb.finalize();
        }
        ok
    };
    if !emitted {
        if dbg {
            eprintln!("t2 codegen-decline {}: body falls off the end", proto.name);
        }
        return None;
    }
    if let Err(e) = module.define_function(fid, &mut clctx) {
        if dbg {
            eprintln!("t2 codegen-decline {}: define_function: {}", proto.name, e);
        }
        return None;
    }
    module.clear_context(&mut clctx);

    // Second pass: the frame-lite / lite-block sibling. A lite failure
    // never fails the whole compile — the framed body still ships.
    let mut lite_ok = false;
    if let (false, Some(lfid)) = (matches!(lite_mode, LiteMode::Off), lite_fid) {
        clctx.func.signature = lite_sig.clone();
        let emitted = {
            let mut fb = FunctionBuilder::new(&mut clctx.func, &mut fbctx);
            let h = make_refs(&mut module, fb.func);
            let ok = emit_body(
                &mut fb, &h, proto, proto_idx, code, n, &leader, ptr_ty, ctx, true, true,
                lite_mode,
            );
            if ok {
                fb.finalize();
            }
            ok
        };
        if emitted {
            match module.define_function(lfid, &mut clctx) {
                Ok(_) => lite_ok = true,
                Err(e) => {
                    if dbg {
                        eprintln!("t2-lite codegen-decline {}: define_function: {}", proto.name, e);
                    }
                }
            }
        } else if dbg {
            eprintln!("t2-lite codegen-decline {}: body falls off the end", proto.name);
        }
        module.clear_context(&mut clctx);
    }

    module.finalize_definitions().ok()?;
    let code_ptr = module.get_finalized_function(fid);
    let ptr = unsafe {
        std::mem::transmute::<*const u8, extern "C" fn(*mut crate::vm::Vm) -> i64>(code_ptr)
    };
    let (lite_ptr, lite_blk_ptr) = match (lite_ok, lite_mode, lite_fid) {
        (true, LiteMode::Method(argc), Some(lfid)) => {
            let p = module.get_finalized_function(lfid);
            (Some((unsafe { std::mem::transmute::<*const u8, T2LiteFn>(p) }, argc)), None)
        }
        (true, LiteMode::Block(ps, np, rest), Some(lfid)) => {
            let p = module.get_finalized_function(lfid);
            (None, Some((unsafe { std::mem::transmute::<*const u8, T2LiteBlkFn>(p) }, ps, np, rest)))
        }
        _ => (None, None),
    };
    Some(T2Proto { _module: module, ptr, lite_ptr, lite_blk_ptr })
}
