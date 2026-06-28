//! End-to-end native (Cranelift) JIT for 1-parameter integer methods.
//! ADR 0030 finding #4: lower a real `Proto` to machine code so a Ruby
//! method call dispatches into native code, with an overflow-guard deopt.
//!
//! Eligibility (else `None`, stays interpreted): exactly one required
//! positional param, no rest/kw, and every op in a small integer set
//! (const/local load+store, +/-/* and comparisons, jumps, return). Any
//! arithmetic overflow OR an arg that isn't an `Int` deopts to the
//! interpreter — so the JIT can never change a result, only its speed.

use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{types, AbiParam, Block, BlockArg, InstBuilder, Value as ClValue};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module};

use crate::bytecode::{BinOpKind, Op, Proto};
use crate::value::Value;

#[repr(C)]
struct NRet {
    res: i64,
    ovf: u8,
}

/// A compiled native 1-param integer method.
pub(crate) struct NativeProto {
    _module: JITModule,
    ptr: extern "C" fn(i64) -> NRet,
}

impl NativeProto {
    /// Run native code on an `Int` arg. `None` = deopt (overflow): the
    /// caller must fall back to the interpreter (which promotes to Bignum).
    #[inline]
    pub(crate) fn call(&self, x: i64) -> Option<i64> {
        let r = (self.ptr)(x);
        if r.ovf == 0 {
            Some(r.res)
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Int,
    Bool,
    Nil,
}

/// Compile an eligible `Proto` to native code, or `None` to keep interpreting.
/// `self_name_id` is the SymId of the method's own name — a no-recv call to it
/// is treated as self-recursion and compiled to a native self-call (the first
/// step toward a call-compiling JIT; polymorphism via an overriding subclass
/// is the next layer, an inline-cache guard).
pub(crate) fn compile(proto: &Proto, self_name_id: crate::intern::SymId) -> Option<NativeProto> {
    // Shape gate: exactly one required positional param, nothing fancy.
    if proto.n_required_positional != 1
        || proto.params.len() != 1
        || proto.rest_param.is_some()
        || !proto.kw_param_defaults.is_empty()
    {
        return None;
    }
    let code = &proto.code;
    // Op gate: every op must be one we model.
    for op in code {
        match op {
            Op::LoadConstInt(_)
            | Op::LoadLocal(_)
            | Op::StoreLocal(_)
            | Op::IncLocal(_)
            | Op::IncLocalNoPush(_)
            | Op::Jump(_)
            | Op::JumpIfFalse(_)
            | Op::Return
            | Op::Pop
            | Op::Dup
            | Op::EnterLoop
            | Op::ExitLoop
            | Op::LoadNil => {}
            Op::BinOp(k) | Op::BinOpLocalLocal(k, _, _) | Op::BinOpInt(k, _) => match k {
                BinOpKind::Add
                | BinOpKind::Sub
                | BinOpKind::Mul
                | BinOpKind::Lt
                | BinOpKind::Le
                | BinOpKind::Gt
                | BinOpKind::Ge
                | BinOpKind::Eq
                | BinOpKind::Ne => {}
                // Div/Mod need floor-semantics + div-by-zero deopt — not modelled yet.
                _ => return None,
            },
            // Self-recursive 1-arg call (`fib(n-1)`): compiled to a native
            // self-call. Any other call shape is ineligible.
            Op::CallNoRecv(name, 1, _) if *name == self_name_id => {}
            _ => return None,
        }
    }

    // Basic-block leaders: ip 0, jump targets, and fall-through after jumps.
    // Jump target = ip + 1 + off (ip is pre-incremented by the dispatch loop).
    let n = code.len();
    let mut leader = vec![false; n + 1];
    leader[0] = true;
    for (ip, op) in code.iter().enumerate() {
        if let Op::Jump(off) | Op::JumpIfFalse(off) = op {
            let t = (ip as i64 + 1 + *off as i64) as usize;
            if t <= n {
                leader[t.min(n)] = true;
            }
            if ip + 1 <= n {
                leader[ip + 1] = true;
            }
        }
    }

    let builder = JITBuilder::new(cranelift_module::default_libcall_names()).ok()?;
    let mut module = JITModule::new(builder);
    let mut ctx = module.make_context();
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I8));
    ctx.func.signature = sig.clone();
    let fid = module.declare_function("m", Linkage::Export, &sig).ok()?;
    let mut fbctx = FunctionBuilderContext::new();
    {
        let mut fb = FunctionBuilder::new(&mut ctx.func, &mut fbctx);
        // A reference to THIS function, for compiling self-recursive calls
        // (`fib(n-1)` → a native call back into the same code).
        let self_ref = module.declare_func_in_func(fid, fb.func);

        // Cranelift block per leader ip; a final `done` for the post-Return tail.
        let mut blocks: Vec<Option<cranelift_codegen::ir::Block>> = vec![None; n + 1];
        for (ip, &is_l) in leader.iter().enumerate() {
            if is_l {
                blocks[ip] = Some(fb.create_block());
            }
        }
        // Entry block: receive param, init locals + overflow var, jump to ip 0.
        let entry = fb.create_block();
        fb.append_block_params_for_function_params(entry);
        fb.switch_to_block(entry);
        let param = fb.block_params(entry)[0];
        let nloc = proto.n_locals as usize;
        let vars: Vec<Variable> = (0..nloc).map(|_| fb.declare_var(types::I64)).collect();
        for (i, v) in vars.iter().enumerate() {
            if i == 0 {
                fb.def_var(*v, param);
            } else {
                let z = fb.ins().iconst(types::I64, 0);
                fb.def_var(*v, z);
            }
        }
        let ovf_var = fb.declare_var(types::I8);
        let z8 = fb.ins().iconst(types::I8, 0);
        fb.def_var(ovf_var, z8);
        fb.ins().jump(blocks[0].unwrap(), &[]);
        fb.seal_block(entry);

        // Per-block codegen with an Int/Bool/Nil kind stack. The operand stack
        // that's live at a basic-block edge flows across via block PARAMETERS
        // (Cranelift's SSA for cross-block values) — `block_kinds[ip]` records
        // the kind of each param, set the first time we branch to that block.
        let mut stack: Vec<(ClValue, Kind)> = Vec::new();
        let mut cur_open = false; // is the current block unterminated?
        let mut block_kinds: Vec<Option<Vec<Kind>>> = vec![None; n + 1];
        let mut ip = 0usize;
        while ip < n {
            if let Some(b) = blocks[ip] {
                // New block leader: pass the live operand stack across the
                // fall-through edge, then re-materialise it from this block's
                // parameters.
                if cur_open {
                    let args = block_args(&mut fb, b, &mut block_kinds[ip], &stack);
                    fb.ins().jump(b, &args);
                }
                fb.switch_to_block(b);
                let kinds = block_kinds[ip].clone().unwrap_or_default();
                let params: Vec<ClValue> = fb.block_params(b).to_vec();
                stack = params.iter().zip(kinds.iter()).map(|(v, k)| (*v, *k)).collect();
                cur_open = true;
            } else if !cur_open {
                // Dead code after a terminator (e.g. the `if`-modifier tail past
                // a `return`) — no reachable block to emit into, so skip it.
                ip += 1;
                continue;
            }
            let acc_ovf = |fb: &mut FunctionBuilder, of: ClValue| {
                let cur = fb.use_var(ovf_var);
                let nv = fb.ins().bor(cur, of);
                fb.def_var(ovf_var, nv);
            };
            match &code[ip] {
                Op::LoadConstInt(i) => {
                    let v = fb.ins().iconst(types::I64, *i);
                    stack.push((v, Kind::Int));
                }
                Op::LoadNil => {
                    // Only ever consumed by Pop in an eligible int method
                    // (a `while` evaluates to nil); a dummy keeps the stack
                    // typed, and the kind guards below reject any real use.
                    let v = fb.ins().iconst(types::I64, 0);
                    stack.push((v, Kind::Nil));
                }
                Op::LoadLocal(s) => {
                    let v = fb.use_var(vars[*s as usize]);
                    stack.push((v, Kind::Int));
                }
                Op::StoreLocal(s) => {
                    let (v, k) = stack.pop()?;
                    if k != Kind::Int {
                        return None;
                    }
                    fb.def_var(vars[*s as usize], v);
                }
                Op::IncLocal(s) | Op::IncLocalNoPush(s) => {
                    let cur = fb.use_var(vars[*s as usize]);
                    let one = fb.ins().iconst(types::I64, 1);
                    let (nv, of) = fb.ins().sadd_overflow(cur, one);
                    acc_ovf(&mut fb, of);
                    fb.def_var(vars[*s as usize], nv);
                    if matches!(&code[ip], Op::IncLocal(_)) {
                        stack.push((nv, Kind::Int));
                    }
                }
                Op::Pop => {
                    stack.pop()?;
                }
                Op::Dup => {
                    let top = *stack.last()?;
                    stack.push(top);
                }
                Op::BinOp(k) => {
                    let (b, kb) = stack.pop()?;
                    let (a, ka) = stack.pop()?;
                    if ka != Kind::Int || kb != Kind::Int {
                        return None;
                    }
                    emit_binop(&mut fb, *k, a, b, &mut stack, ovf_var);
                }
                Op::BinOpLocalLocal(k, a_slot, b_slot) => {
                    let a = fb.use_var(vars[*a_slot as usize]);
                    let b = fb.use_var(vars[*b_slot as usize]);
                    emit_binop(&mut fb, *k, a, b, &mut stack, ovf_var);
                }
                Op::BinOpInt(k, imm) => {
                    let (a, _) = stack.pop()?;
                    let b = fb.ins().iconst(types::I64, *imm);
                    emit_binop(&mut fb, *k, a, b, &mut stack, ovf_var);
                }
                Op::Jump(off) => {
                    let t = (ip as i64 + 1 + *off as i64) as usize;
                    let tb = blocks[t].unwrap();
                    let args = block_args(&mut fb, tb, &mut block_kinds[t], &stack);
                    fb.ins().jump(tb, &args);
                    cur_open = false;
                }
                Op::JumpIfFalse(off) => {
                    let (cond, k) = stack.pop()?;
                    if k != Kind::Bool {
                        return None; // only comparison conditions modelled
                    }
                    let t = (ip as i64 + 1 + *off as i64) as usize;
                    let fall = blocks[ip + 1].unwrap();
                    let target = blocks[t].unwrap();
                    // The stack remaining after popping `cond` flows to BOTH
                    // successors (same depth/kinds).
                    let fall_args = block_args(&mut fb, fall, &mut block_kinds[ip + 1], &stack);
                    let target_args = block_args(&mut fb, target, &mut block_kinds[t], &stack);
                    // brif: non-zero (true) -> fall-through, zero (false) -> target.
                    fb.ins().brif(cond, fall, &fall_args, target, &target_args);
                    cur_open = false;
                }
                Op::Return => {
                    let (v, _) = stack.pop()?;
                    let ov = fb.use_var(ovf_var);
                    fb.ins().return_(&[v, ov]);
                    cur_open = false;
                }
                // Self-recursive call: pop the i64 arg, emit a native call to
                // this same function, push the result, OR the callee's overflow
                // flag into ours (so a deep overflow deopts the whole tree).
                Op::CallNoRecv(name, 1, _) if *name == self_name_id => {
                    let (arg, ka) = stack.pop()?;
                    if ka != Kind::Int {
                        return None;
                    }
                    let inst = fb.ins().call(self_ref, &[arg]);
                    let (res, ovf) = {
                        let r = fb.inst_results(inst);
                        (r[0], r[1])
                    };
                    let cur = fb.use_var(ovf_var);
                    let nv = fb.ins().bor(cur, ovf);
                    fb.def_var(ovf_var, nv);
                    stack.push((res, Kind::Int));
                }
                Op::EnterLoop | Op::ExitLoop => {} // interpreter loop-stack bookkeeping; no native state
                _ => return None,
            }
            ip += 1;
        }
        fb.seal_all_blocks();
        fb.finalize();
    }
    module.define_function(fid, &mut ctx).ok()?;
    module.clear_context(&mut ctx);
    module.finalize_definitions().ok()?;
    let code_ptr = module.get_finalized_function(fid);
    let ptr = unsafe { std::mem::transmute::<_, extern "C" fn(i64) -> NRet>(code_ptr) };
    Some(NativeProto { _module: module, ptr })
}

/// Compute the block-call arguments for branching to `block` with the current
/// operand `stack` live. The FIRST branch to a block fixes its parameter count
/// + kinds (`kinds_slot`); later branches must arrive with the same shape
/// (true for structured bytecode). Returns the SSA values to pass as args.
fn block_args(
    fb: &mut FunctionBuilder,
    block: Block,
    kinds_slot: &mut Option<Vec<Kind>>,
    stack: &[(ClValue, Kind)],
) -> Vec<BlockArg> {
    if kinds_slot.is_none() {
        for _ in stack {
            fb.append_block_param(block, types::I64);
        }
        *kinds_slot = Some(stack.iter().map(|(_, k)| *k).collect());
    }
    stack.iter().map(|(v, _)| (*v).into()).collect()
}

fn emit_binop(
    fb: &mut FunctionBuilder,
    k: BinOpKind,
    a: ClValue,
    b: ClValue,
    stack: &mut Vec<(ClValue, Kind)>,
    ovf_var: Variable,
) {
    let cc = |k: BinOpKind| match k {
        BinOpKind::Lt => Some(IntCC::SignedLessThan),
        BinOpKind::Le => Some(IntCC::SignedLessThanOrEqual),
        BinOpKind::Gt => Some(IntCC::SignedGreaterThan),
        BinOpKind::Ge => Some(IntCC::SignedGreaterThanOrEqual),
        BinOpKind::Eq => Some(IntCC::Equal),
        BinOpKind::Ne => Some(IntCC::NotEqual),
        _ => None,
    };
    if let Some(cond) = cc(k) {
        let r = fb.ins().icmp(cond, a, b);
        stack.push((r, Kind::Bool));
        return;
    }
    let (res, of) = match k {
        BinOpKind::Add => fb.ins().sadd_overflow(a, b),
        BinOpKind::Sub => fb.ins().ssub_overflow(a, b),
        BinOpKind::Mul => fb.ins().smul_overflow(a, b),
        _ => unreachable!("non-arith binop reached emit"),
    };
    let cur = fb.use_var(ovf_var);
    let nv = fb.ins().bor(cur, of);
    fb.def_var(ovf_var, nv);
    stack.push((res, Kind::Int));
}

// ---- D Layer 3: value-representation method JIT ----
//
// The integer JIT above unboxes `Int` locals to raw i64. To compile a method
// that handles arbitrary `Value`s (AR's attribute access, etc.) the JIT instead
// passes Values BY POINTER and calls rubyrs primitives natively. This first
// value-method shape is the attr reader `def v; @v; end` — its native code is a
// single call to `jit_ivar_get` with the ivar name baked in.

/// A compiled value-method: `fn(vm, recv, out)` — reads from `recv` (a `Value`
/// pointer) and writes the result `Value` to `out`. Holds the `JITModule`.
pub(crate) struct ValueProto {
    _module: JITModule,
    ptr: extern "C" fn(*const crate::vm::Vm, *const Value, *mut Value),
}

impl ValueProto {
    /// Run the native value-method: `recv` is the receiver, the result is
    /// returned (written through an out-slot). `vm` is borrowed shared for the
    /// duration of the call (the primitive only reads the heap).
    #[inline]
    pub(crate) fn call(&self, vm: *const crate::vm::Vm, recv: &Value) -> Value {
        let mut out = Value::Nil;
        (self.ptr)(vm, recv as *const Value, &mut out as *mut Value);
        out
    }
}

/// Native primitive: read `recv`'s ivar `name`, write it to `out`. The seam a
/// value-method JIT calls — `Value` crosses by pointer (no enum layout in
/// codegen). Read-only on the heap, so `vm` is shared.
///
/// # Safety
/// `vm`, `recv`, `out` must be valid, and `*vm` must outlive the call.
pub(crate) unsafe extern "C" fn jit_ivar_get(
    vm: *const crate::vm::Vm,
    recv: *const Value,
    name: u32,
    out: *mut Value,
) {
    let vm = unsafe { &*vm };
    let recv = unsafe { &*recv };
    let name_id = crate::intern::SymId(name);
    let v = match recv {
        Value::Object(oid) => match vm.heap.get(*oid) {
            crate::heap::HeapObj::Instance(inst) => {
                inst.ivars.get(&name_id).cloned().unwrap_or(Value::Nil)
            }
            _ => Value::Nil,
        },
        Value::Class(cls) => cls.ivars.borrow().get(&name_id).cloned().unwrap_or(Value::Nil),
        _ => Value::Nil,
    };
    unsafe { std::ptr::write(out, v) };
}

/// Compile an attr reader (`def v; @v; end`, body `[LoadIvar(sym), Return]`) to
/// native code that calls `jit_ivar_get` with `getter_sym` baked in.
pub(crate) fn compile_attr_reader(getter_sym: crate::intern::SymId) -> Option<ValueProto> {
    let mut builder = JITBuilder::new(cranelift_module::default_libcall_names()).ok()?;
    builder.symbol("jit_ivar_get", jit_ivar_get as *const u8);
    let mut module = JITModule::new(builder);
    let ptr_ty = module.target_config().pointer_type();
    let mut ctx = module.make_context();
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(ptr_ty)); // vm
    sig.params.push(AbiParam::new(ptr_ty)); // recv (*const Value)
    sig.params.push(AbiParam::new(ptr_ty)); // out  (*mut Value)
    ctx.func.signature = sig.clone();
    let fid = module.declare_function("g", Linkage::Export, &sig).ok()?;
    let mut hsig = module.make_signature();
    hsig.params.push(AbiParam::new(ptr_ty)); // vm
    hsig.params.push(AbiParam::new(ptr_ty)); // recv
    hsig.params.push(AbiParam::new(types::I32)); // name
    hsig.params.push(AbiParam::new(ptr_ty)); // out
    let hid = module.declare_function("jit_ivar_get", Linkage::Import, &hsig).ok()?;
    let mut fbctx = FunctionBuilderContext::new();
    {
        let mut fb = FunctionBuilder::new(&mut ctx.func, &mut fbctx);
        let href = module.declare_func_in_func(hid, fb.func);
        let b = fb.create_block();
        fb.append_block_params_for_function_params(b);
        fb.switch_to_block(b);
        fb.seal_block(b);
        let vm = fb.block_params(b)[0];
        let recv = fb.block_params(b)[1];
        let out = fb.block_params(b)[2];
        let name = fb.ins().iconst(types::I32, getter_sym.0 as i64);
        fb.ins().call(href, &[vm, recv, name, out]);
        fb.ins().return_(&[]);
        fb.finalize();
    }
    module.define_function(fid, &mut ctx).ok()?;
    module.clear_context(&mut ctx);
    module.finalize_definitions().ok()?;
    let code = module.get_finalized_function(fid);
    let ptr = unsafe {
        std::mem::transmute::<_, extern "C" fn(*const crate::vm::Vm, *const Value, *mut Value)>(code)
    };
    Some(ValueProto { _module: module, ptr })
}

/// Extract an i64 from an `Int` value (the guard before calling native).
#[inline]
pub(crate) fn as_int(v: &Value) -> Option<i64> {
    match v {
        Value::Int(n) => Some(*n),
        _ => None,
    }
}
