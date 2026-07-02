//! Tier-2 baseline JIT: the FRAME-KEEPING DIRECT-THREADED tier (ADR 0037).
//!
//! Compiles a method body's op SEQUENCE to native code that keeps the REAL
//! interpreter frame (locals, self, base_sp, ip) and the real operand stack,
//! but eliminates the interpreter's per-op fetch/decode/dispatch loop:
//!
//! - branch targets become native jumps (no ip arithmetic, no re-fetch),
//! - op operands become immediates baked into specialized helper calls,
//! - the hot simple ops (local/ivar/literal loads, stores, stack shuffles)
//!   call tiny per-op-kind helpers that mirror `Vm::step`'s arms exactly,
//! - every other admitted op runs through ONE generic helper that executes
//!   the interpreter's own `step()` for that op — so per-op semantics are
//!   the interpreter's by construction — and, when the op pushed a callee
//!   frame, drives it to completion with `dispatch_until` (the same nested-
//!   driver pattern the Rust iterator primitives use).
//!
//! THE correctness property (why this tier needs NO deopt discipline): at
//! every op boundary the machine state is EXACTLY the interpreter's state —
//! real frame, real operand stack, and `frame.ip` maintained ahead of every
//! effectful op. Bailing out mid-body (a control signal, a fiber yield, a
//! pending non-local return) is therefore always safe: the native code simply
//! returns and the interpreter CONTINUES the frame at `ip` — a mode switch,
//! not a re-execution. Side effects never replay.
//!
//! Traps: a raising op (or a raising callee) propagates its `Trap` through
//! `Vm::t2_trap` and status 3; the serving site re-`Err`s it, and the outer
//! dispatch loop runs the exact rescue/unwind machinery it would have run for
//! the interpreted frame (our frame's `ip` is current, so backtrace spans are
//! byte-identical). `AlreadyCaught` flows through unchanged — same contract
//! as every nested `dispatch_until` driver.
//!
//! Admission declines only the ops that could redirect `ip` INTO this frame
//! behind the native code's back (rescue/ensure installation + raise/re-raise
//! terminators) and the non-local-exit ops whose owner semantics need the
//! master loop (`ReturnMethod`, block `Break`). Everything else — including
//! calls, blocks (`CreateBlock`/`CallBlock`/`Yield`), massign splats,
//! constants, globals — is admitted.

use cranelift_codegen::ir::{types, AbiParam, Block, BlockArg, InstBuilder};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module};

use crate::bytecode::{Op, Proto};
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

/// A compiled tier-2 method body: `(vm) -> status`. Runs the frame currently
/// on top of `vm.frames` (which the serving site just pushed) to completion,
/// or bails/traps with the frame state consistent for the interpreter.
pub(crate) struct T2Proto {
    _module: JITModule,
    pub(crate) ptr: extern "C" fn(*mut crate::vm::Vm) -> i64,
}

// ---------------------------------------------------------------------------
// Runtime helpers. All take `vm: *mut Vm`; the native code holds no Rust
// references, so reconstructing `&mut Vm` here is sound (same discipline as
// the jit_native primitives). GC safety: every Value lives in `vm.stack` /
// frame locals (real GC roots) — the native code never holds a Value.
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
    if let Err(t) = vm.step(op, pidx) {
        vm.t2_trap = Some(t);
        return T2_TRAP;
    }
    t2_finish(vm, depth)
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
    if vm.frames.len() > depth {
        if let Err(t) = vm.dispatch_until(depth) {
            vm.t2_trap = Some(t);
            return T2_TRAP;
        }
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
#[inline]
fn t2_call_impl(
    vm: &mut crate::vm::Vm,
    name_id: SymId,
    argc: usize,
    cache_id: u32,
    no_recv: bool,
) -> i64 {
    let depth = vm.frames.len();
    let fast = !vm.bypass_visibility_once
        && !vm.force_primitive_dispatch
        && (vm.refined_method_names.is_empty() || !vm.refined_method_names.contains(&name_id));
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
            // Receiver-typed fast serves, in `do_call`'s exact order. The
            // Str/Array/Block/Hash per-instance singleton gates and the
            // `proc.call` arm sit BETWEEN the boundary gates and these
            // helpers in the cascade; both are inert here by the guards
            // below (no singletons of those kinds exist / the name isn't
            // `call`), so skipping straight to the helpers is exact — and
            // every helper is a no-op on miss.
            let singleton_free = !vm.any_str_singletons
                && !vm.any_heap_singletons
                && !vm.any_hash_singletons
                && name_id != vm.sym_call;
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
                    Ok(false) => vm.try_invoke_class_singleton_cached(name_id, argc, cache_id),
                    r => r,
                }
            }
        };
        vm.trailing_hash_positional = false;
        match served {
            Ok(true) => {
                if vm.jit_stats_on {
                    vm.t2_call_stats[0] += 1;
                }
                return t2_finish(vm, depth);
            }
            Ok(false) => {}
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
    let v = match &f.locals {
        crate::vm::Locals::Stack(base) => {
            vm.locals_arena[*base as usize + slot as usize].clone()
        }
        crate::vm::Locals::Shared(rc) => rc.borrow()[slot as usize].clone(),
    };
    vm.stack.push(v);
    t2_call_impl(vm, SymId(name as u32), 0, cid as u32, false)
}

/// `Op::Return` — the frame-pop shortcut (wave 2): mirrors `step()`'s
/// `Op::Return` arm without the fetch/decode/match round-trip, so a
/// native→native callee returns straight to its caller's native code. The
/// two cold shapes — a pending `is_ensure` handler (unreachable for an
/// admitted body: admission declines `PushEnsure`; kept for exactness) and a
/// class-body frame (tier-2 frames are method frames by construction) —
/// route through the interpreter's own arm via `step`. The hot path is the
/// arm's plain direct-pop, byte for byte: `$~` restore, `$!` restore, pop +
/// truncate-to-`base_sp` + push return (honoring `swap_return`), then the
/// `release_frame_locals` / `recycle_frame_aux` recycling discipline
/// (3397804a).
unsafe extern "C" fn t2_return(vm: *mut crate::vm::Vm, pidx: i64, ip: i64) -> i64 {
    let vm = unsafe { &mut *vm };
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
        // `step` charges its own fuel tick.
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
    let ret = vm.stack.pop().unwrap_or(Value::Nil);
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
    let v = match &f.locals {
        crate::vm::Locals::Stack(base) => {
            vm.locals_arena[*base as usize + slot as usize].clone()
        }
        crate::vm::Locals::Shared(rc) => rc.borrow()[slot as usize].clone(),
    };
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
            rc.borrow_mut()[slot] = v.clone();
            // Method frames never carry `block_writeback`; mirrored for
            // exactness with the step arm anyway.
            let in_outer_scope = frame
                .block_writeback
                .as_ref()
                .is_some_and(|(_, ps)| slot < *ps as usize);
            if in_outer_scope {
                vm.propagate_outer_write(slot, &v);
            }
        }
    }
}

unsafe extern "C" fn t2_load_ivar(vm: *mut crate::vm::Vm, name_id: i64) {
    let vm = unsafe { &mut *vm };
    let name_id = SymId(name_id as u32);
    let self_val = vm.frames.last().expect("ICE: LoadIvar no frame").self_val.clone();
    let v = match &self_val {
        Value::Object(id) => vm
            .heap
            .instance(*id)
            .ivars
            .get(&name_id)
            .cloned()
            .unwrap_or(Value::Nil),
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
            Op::Jump(off) | Op::BreakLoop(off) | Op::NextLoop(off) => {
                if jump_target(i, *off) >= n {
                    return Err(format!("jump target out of range at {}", i));
                }
            }
            Op::JumpIfFalse(off) | Op::JumpIfArgGiven(_, off) | Op::JumpIfKwArgGiven(_, off) => {
                if jump_target(i, *off) >= n || i + 1 >= n {
                    return Err(format!("cond target out of range at {}", i));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Compile `proto`'s body to a tier-2 native function. Returns `None` when
/// the body is not admitted (or codegen fails) — the caller records the
/// verdict and keeps interpreting. `nocall` (env `RUBYRS_JIT_TIER2_NOCALL`)
/// disables the wave-2 IC-fast call/return helpers — every call op and
/// `Return` compiles through the generic `t2_op` helper, reproducing the
/// wave-1 tier for controlled A/B runs.
pub(crate) fn compile_tier2(proto: &Proto, proto_idx: usize, nocall: bool) -> Option<T2Proto> {
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
            Op::Return => {
                if i + 1 < n {
                    leader[i + 1] = true;
                }
            }
            _ => {}
        }
    }

    let mut builder = JITBuilder::new(cranelift_module::default_libcall_names()).ok()?;
    builder.symbol("t2_op", t2_op as *const u8);
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
    builder.symbol("t2_dup", t2_dup as *const u8);
    builder.symbol("t2_pop", t2_pop as *const u8);
    builder.symbol("t2_swap", t2_swap as *const u8);
    builder.symbol("t2_call", t2_call as *const u8);
    builder.symbol("t2_call_norecv", t2_call_norecv as *const u8);
    builder.symbol("t2_call_local", t2_call_local as *const u8);
    builder.symbol("t2_return", t2_return as *const u8);
    let mut module = JITModule::new(builder);
    let ptr_ty = module.target_config().pointer_type();

    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(ptr_ty)); // vm
    sig.returns.push(AbiParam::new(types::I64)); // status
    let mut ctx = module.make_context();
    ctx.func.signature = sig.clone();
    let fid = module.declare_function("t2body", Linkage::Export, &sig).ok()?;

    // Helper signatures.
    let sig_vm_i64_i64_ret = {
        let mut s = module.make_signature();
        s.params.push(AbiParam::new(ptr_ty)); // vm
        s.params.push(AbiParam::new(ptr_ty)); // *const Op (baked)
        s.params.push(AbiParam::new(types::I64)); // pidx
        s.params.push(AbiParam::new(types::I64)); // ip
        s.returns.push(AbiParam::new(types::I64));
        s
    };
    let sig_vm_ret = {
        let mut s = module.make_signature();
        s.params.push(AbiParam::new(ptr_ty));
        s.returns.push(AbiParam::new(types::I64));
        s
    };
    let sig_vm_i64_ret = {
        let mut s = module.make_signature();
        s.params.push(AbiParam::new(ptr_ty));
        s.params.push(AbiParam::new(types::I64));
        s.returns.push(AbiParam::new(types::I64));
        s
    };
    let sig_vm = {
        let mut s = module.make_signature();
        s.params.push(AbiParam::new(ptr_ty));
        s
    };
    let sig_vm_i64 = {
        let mut s = module.make_signature();
        s.params.push(AbiParam::new(ptr_ty));
        s.params.push(AbiParam::new(types::I64));
        s
    };
    // (vm, a, b, c, d) -> status — the t2_call family.
    let sig_vm_4i64_ret = {
        let mut s = module.make_signature();
        s.params.push(AbiParam::new(ptr_ty));
        for _ in 0..4 {
            s.params.push(AbiParam::new(types::I64));
        }
        s.returns.push(AbiParam::new(types::I64));
        s
    };
    // (vm, pidx, ip) -> status — t2_return.
    let sig_vm_2i64_ret = {
        let mut s = module.make_signature();
        s.params.push(AbiParam::new(ptr_ty));
        for _ in 0..2 {
            s.params.push(AbiParam::new(types::I64));
        }
        s.returns.push(AbiParam::new(types::I64));
        s
    };

    let f_op = module.declare_function("t2_op", Linkage::Import, &sig_vm_i64_i64_ret).ok()?;
    let f_pop_truthy = module.declare_function("t2_pop_truthy", Linkage::Import, &sig_vm_ret).ok()?;
    let f_arg_given = module.declare_function("t2_arg_given", Linkage::Import, &sig_vm_i64_ret).ok()?;
    let f_kwarg_given = module.declare_function("t2_kwarg_given", Linkage::Import, &sig_vm_i64_ret).ok()?;
    let f_push_int = module.declare_function("t2_push_int", Linkage::Import, &sig_vm_i64).ok()?;
    let f_push_nil = module.declare_function("t2_push_nil", Linkage::Import, &sig_vm).ok()?;
    let f_push_bool = module.declare_function("t2_push_bool", Linkage::Import, &sig_vm_i64).ok()?;
    let f_push_sym = module.declare_function("t2_push_sym", Linkage::Import, &sig_vm_i64).ok()?;
    let f_load_self = module.declare_function("t2_load_self", Linkage::Import, &sig_vm).ok()?;
    let f_load_local = module.declare_function("t2_load_local", Linkage::Import, &sig_vm_i64).ok()?;
    let f_store_local = module.declare_function("t2_store_local", Linkage::Import, &sig_vm_i64).ok()?;
    let f_load_ivar = module.declare_function("t2_load_ivar", Linkage::Import, &sig_vm_i64).ok()?;
    let f_dup = module.declare_function("t2_dup", Linkage::Import, &sig_vm).ok()?;
    let f_pop = module.declare_function("t2_pop", Linkage::Import, &sig_vm).ok()?;
    let f_swap = module.declare_function("t2_swap", Linkage::Import, &sig_vm).ok()?;
    let f_call = module.declare_function("t2_call", Linkage::Import, &sig_vm_4i64_ret).ok()?;
    let f_call_norecv =
        module.declare_function("t2_call_norecv", Linkage::Import, &sig_vm_4i64_ret).ok()?;
    let f_call_local =
        module.declare_function("t2_call_local", Linkage::Import, &sig_vm_4i64_ret).ok()?;
    let f_return = module.declare_function("t2_return", Linkage::Import, &sig_vm_2i64_ret).ok()?;

    let mut fbctx = FunctionBuilderContext::new();
    {
        let mut fb = FunctionBuilder::new(&mut ctx.func, &mut fbctx);
        let r_op = module.declare_func_in_func(f_op, fb.func);
        let r_pop_truthy = module.declare_func_in_func(f_pop_truthy, fb.func);
        let r_arg_given = module.declare_func_in_func(f_arg_given, fb.func);
        let r_kwarg_given = module.declare_func_in_func(f_kwarg_given, fb.func);
        let r_push_int = module.declare_func_in_func(f_push_int, fb.func);
        let r_push_nil = module.declare_func_in_func(f_push_nil, fb.func);
        let r_push_bool = module.declare_func_in_func(f_push_bool, fb.func);
        let r_push_sym = module.declare_func_in_func(f_push_sym, fb.func);
        let r_load_self = module.declare_func_in_func(f_load_self, fb.func);
        let r_load_local = module.declare_func_in_func(f_load_local, fb.func);
        let r_store_local = module.declare_func_in_func(f_store_local, fb.func);
        let r_load_ivar = module.declare_func_in_func(f_load_ivar, fb.func);
        let r_dup = module.declare_func_in_func(f_dup, fb.func);
        let r_pop = module.declare_func_in_func(f_pop, fb.func);
        let r_swap = module.declare_func_in_func(f_swap, fb.func);
        let r_call = module.declare_func_in_func(f_call, fb.func);
        let r_call_norecv = module.declare_func_in_func(f_call_norecv, fb.func);
        let r_call_local = module.declare_func_in_func(f_call_local, fb.func);
        let r_return = module.declare_func_in_func(f_return, fb.func);

        let entry = fb.create_block();
        fb.append_block_params_for_function_params(entry);
        // Exit block: returns the status carried as its block param.
        let exit = fb.create_block();
        fb.append_block_param(exit, types::I64);
        let blocks: Vec<Option<Block>> = leader
            .iter()
            .map(|&l| if l { Some(fb.create_block()) } else { None })
            .collect();

        fb.switch_to_block(entry);
        let vm = fb.block_params(entry)[0];
        fb.ins().jump(blocks[0].unwrap(), &[]);

        // The entry block is terminated; the first loop iteration switches
        // into blocks[0].
        let mut terminated = true;
        for i in 0..n {
            if let Some(b) = blocks[i] {
                if !terminated {
                    fb.ins().jump(b, &[]);
                }
                fb.switch_to_block(b);
                terminated = false;
            }
            if terminated {
                // Unreachable op (between a terminator and the next leader).
                continue;
            }
            let ipc = fb.ins().iconst(types::I64, i as i64);
            let pidxc = fb.ins().iconst(types::I64, proto_idx as i64);
            // Baked pointer to this op inside the proto's stable code buffer
            // (skips the protos[pidx].code[ip] fetch chain at run time).
            let opp = fb
                .ins()
                .iconst(ptr_ty, unsafe { code.as_ptr().add(i) } as i64);
            // Status-check emitter: brif status==CONTINUE → cont else exit.
            macro_rules! check_status {
                ($fb:expr, $st:expr) => {{
                    let cont = $fb.create_block();
                    let is_ok = $fb.ins().icmp_imm(
                        cranelift_codegen::ir::condcodes::IntCC::Equal,
                        $st,
                        T2_CONTINUE,
                    );
                    $fb.ins()
                        .brif(is_ok, cont, &[], exit, &[BlockArg::Value($st)]);
                    $fb.switch_to_block(cont);
                }};
            }
            match &code[i] {
                Op::Jump(off) => {
                    fb.ins().jump(blocks[jump_target(i, *off)].unwrap(), &[]);
                    terminated = true;
                }
                Op::JumpIfFalse(off) => {
                    let call = fb.ins().call(r_pop_truthy, &[vm]);
                    let truthy = fb.inst_results(call)[0];
                    let tgt = blocks[jump_target(i, *off)].unwrap();
                    let cont = blocks[i + 1].unwrap();
                    fb.ins().brif(truthy, cont, &[], tgt, &[]);
                    terminated = true;
                }
                Op::JumpIfArgGiven(slot, off) => {
                    let s = fb.ins().iconst(types::I64, *slot as i64);
                    let call = fb.ins().call(r_arg_given, &[vm, s]);
                    let given = fb.inst_results(call)[0];
                    let tgt = blocks[jump_target(i, *off)].unwrap();
                    let cont = blocks[i + 1].unwrap();
                    fb.ins().brif(given, tgt, &[], cont, &[]);
                    terminated = true;
                }
                Op::JumpIfKwArgGiven(kw_idx, off) => {
                    let s = fb.ins().iconst(types::I64, *kw_idx as i64);
                    let call = fb.ins().call(r_kwarg_given, &[vm, s]);
                    let given = fb.inst_results(call)[0];
                    let tgt = blocks[jump_target(i, *off)].unwrap();
                    let cont = blocks[i + 1].unwrap();
                    fb.ins().brif(given, tgt, &[], cont, &[]);
                    terminated = true;
                }
                Op::Return => {
                    // Wave 2: the dedicated frame-pop shortcut (mirrors the
                    // step arm; cold ensure/class-body shapes route through
                    // `step` inside the helper). Whatever status comes back
                    // (DONE on the pop; TRAP propagates) is the function's
                    // result — a native→native callee returns straight to
                    // its caller's native code here.
                    let st = if nocall {
                        let call = fb.ins().call(r_op, &[vm, opp, pidxc, ipc]);
                        fb.inst_results(call)[0]
                    } else {
                        let call = fb.ins().call(r_return, &[vm, pidxc, ipc]);
                        fb.inst_results(call)[0]
                    };
                    fb.ins().return_(&[st]);
                    terminated = true;
                }
                // --- wave-2 IC-fast call family (plain fixed-argc forms;
                // the walk census: explicit-recv + self-recv argc 0-2 are
                // 84% of frames). Kw / block / splat / send forms stay on
                // the generic helper below. ---
                Op::Call(name, argc, cid) if !nocall && *argc <= 2 => {
                    let n = fb.ins().iconst(types::I64, name.0 as i64);
                    let a = fb.ins().iconst(types::I64, *argc as i64);
                    let c = fb.ins().iconst(types::I64, *cid as i64);
                    let call = fb.ins().call(r_call, &[vm, n, a, c, ipc]);
                    let st = fb.inst_results(call)[0];
                    check_status!(fb, st);
                }
                Op::CallNoRecv(name, argc, cid) if !nocall && *argc <= 2 => {
                    let n = fb.ins().iconst(types::I64, name.0 as i64);
                    let a = fb.ins().iconst(types::I64, *argc as i64);
                    let c = fb.ins().iconst(types::I64, *cid as i64);
                    let call = fb.ins().call(r_call_norecv, &[vm, n, a, c, ipc]);
                    let st = fb.inst_results(call)[0];
                    check_status!(fb, st);
                }
                Op::LoadLocalCall(slot, name, cid) if !nocall => {
                    let s = fb.ins().iconst(types::I64, *slot as i64);
                    let n = fb.ins().iconst(types::I64, name.0 as i64);
                    let c = fb.ins().iconst(types::I64, *cid as i64);
                    let call = fb.ins().call(r_call_local, &[vm, s, n, c, ipc]);
                    let st = fb.inst_results(call)[0];
                    check_status!(fb, st);
                }
                Op::BreakLoop(off) | Op::NextLoop(off) => {
                    // Run the step arm (loop bookkeeping + its own ip
                    // retarget), then take the SAME edge natively.
                    let call = fb.ins().call(r_op, &[vm, opp, pidxc, ipc]);
                    let st = fb.inst_results(call)[0];
                    check_status!(fb, st);
                    fb.ins().jump(blocks[jump_target(i, *off)].unwrap(), &[]);
                    terminated = true;
                }
                // --- specialized call-free, trap-free, signal-free ops ---
                Op::LoadConstInt(v) => {
                    let c = fb.ins().iconst(types::I64, *v);
                    fb.ins().call(r_push_int, &[vm, c]);
                }
                Op::LoadNil => {
                    fb.ins().call(r_push_nil, &[vm]);
                }
                Op::LoadTrue => {
                    let c = fb.ins().iconst(types::I64, 1);
                    fb.ins().call(r_push_bool, &[vm, c]);
                }
                Op::LoadFalse => {
                    let c = fb.ins().iconst(types::I64, 0);
                    fb.ins().call(r_push_bool, &[vm, c]);
                }
                Op::LoadSymbol(id) => {
                    let c = fb.ins().iconst(types::I64, id.0 as i64);
                    fb.ins().call(r_push_sym, &[vm, c]);
                }
                Op::LoadSelf => {
                    fb.ins().call(r_load_self, &[vm]);
                }
                Op::LoadLocal(s) => {
                    let c = fb.ins().iconst(types::I64, *s as i64);
                    fb.ins().call(r_load_local, &[vm, c]);
                }
                Op::StoreLocal(s) => {
                    let c = fb.ins().iconst(types::I64, *s as i64);
                    fb.ins().call(r_store_local, &[vm, c]);
                }
                Op::LoadIvar(sym) => {
                    let c = fb.ins().iconst(types::I64, sym.0 as i64);
                    fb.ins().call(r_load_ivar, &[vm, c]);
                }
                Op::Dup => {
                    fb.ins().call(r_dup, &[vm]);
                }
                Op::Pop => {
                    fb.ins().call(r_pop, &[vm]);
                }
                Op::Swap => {
                    fb.ins().call(r_swap, &[vm]);
                }
                // --- everything else: full interpreter semantics ---
                _ => {
                    let call = fb.ins().call(r_op, &[vm, opp, pidxc, ipc]);
                    let st = fb.inst_results(call)[0];
                    check_status!(fb, st);
                }
            }
        }
        if !terminated {
            // A well-formed proto ends in a terminator; refuse otherwise.
            if dbg {
                eprintln!("t2 codegen-decline {}: body falls off the end", proto.name);
            }
            return None;
        }
        fb.switch_to_block(exit);
        let st = fb.block_params(exit)[0];
        fb.ins().return_(&[st]);
        fb.seal_all_blocks();
        fb.finalize();
    }
    if let Err(e) = module.define_function(fid, &mut ctx) {
        if dbg {
            eprintln!("t2 codegen-decline {}: define_function: {}", proto.name, e);
        }
        return None;
    }
    module.clear_context(&mut ctx);
    module.finalize_definitions().ok()?;
    let code_ptr = module.get_finalized_function(fid);
    let ptr = unsafe {
        std::mem::transmute::<*const u8, extern "C" fn(*mut crate::vm::Vm) -> i64>(code_ptr)
    };
    Some(T2Proto { _module: module, ptr })
}
