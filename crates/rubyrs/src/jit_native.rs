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
use cranelift_codegen::ir::{types, AbiParam, Block, BlockArg, FuncRef, InstBuilder, Value as ClValue};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module};

use crate::bytecode::{BinOpKind, Op, Proto};
use crate::intern::{FxHashMap, SymId};
use crate::value::Value;

#[repr(C)]
pub(crate) struct NRet {
    res: i64,
    ovf: u8,
}

/// A compiled native 1-param integer method. The convention is
/// `(vm, self, i64_arg) -> (i64, ovf)`: `vm` + `self` are threaded so the body
/// can call primitives that touch the heap (e.g. read an `Int` ivar) — unused
/// by pure-integer methods, the foundation for value-touching call trees.
pub(crate) struct NativeProto {
    _module: JITModule,
    ptr: extern "C" fn(*const crate::vm::Vm, *const Value, i64) -> NRet,
    /// Polymorphism guard: if non-zero, this code baked in cross-method callees
    /// resolved on ONE specific receiver class (the `Rc<Class>` pointer), so it
    /// is only valid when dispatched on that class — a subclass that overrides a
    /// callee must NOT use it. 0 = no baked cross-calls, valid for any receiver.
    pub(crate) guard_class: std::cell::Cell<usize>,
    /// When true the i64 result is an Array `ObjId` — the dispatch boxes it to
    /// `Value::Array` instead of `Value::Int`.
    pub(crate) returns_array: std::cell::Cell<bool>,
    /// Monomorphic inline caches for `@arr[i].getter` sites (B4). Each holds
    /// `(element_class_ptr, ivar_sym)`; their stable heap addresses are baked
    /// into the native code as constants, so the `Box`es must outlive the code —
    /// they're owned here and dropped with the proto (and its module).
    _caches: Vec<Box<std::cell::Cell<(usize, u32)>>>,
}

impl NativeProto {
    /// Run native code. `recv` is the receiver (read by ivar primitives; ignored
    /// by pure-integer methods). `None` = deopt (overflow, or a non-Int ivar):
    /// the caller falls back to the interpreter.
    #[inline]
    pub(crate) fn call(&self, vm: *const crate::vm::Vm, recv: &Value, x: i64) -> Option<i64> {
        let r = (self.ptr)(vm, recv as *const Value, x);
        if r.ovf == 0 {
            Some(r.res)
        } else {
            None
        }
    }

    /// Machine address of the compiled code — registered as a JIT symbol so a
    /// DIFFERENT method's compilation can emit a native call to this one
    /// (cross-method call compilation; the receiver is assumed monomorphic,
    /// invalidated on `method_gen` like every other cache).
    #[inline]
    pub(crate) fn addr(&self) -> usize {
        self.ptr as usize
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Int,
    Bool,
    Nil,
    /// An i64 that is actually an Array's `ObjId` — a value-local array built
    /// in-method (`a = []; a << x`). Boxed to `Value::Array` at dispatch. GC-safe
    /// because the JIT primitives never call `maybe_gc`, so no collection runs
    /// during the method (the array is rooted by the caller's stack on return).
    ArrayObjId,
}

/// Compile an eligible `Proto` to native code, or `None` to keep interpreting.
/// `self_name_id` is the SymId of the method's own name — a no-recv call to it
/// is a self-recursive native call. `callees` maps the name of an
/// already-compiled 1-arg integer method to its machine address, so a no-recv
/// call to ANOTHER such method also compiles to a native call — this is the
/// "compilation scope" step: a method's whole call tree can run native (like
/// `fib`), not just the leaf. Polymorphism (an overriding subclass) is guarded
/// only by `method_gen` invalidation for now.
/// `getters` maps the name of a 0-arg call (on `self`) that resolves to a simple
/// int-returning attribute reader (`def amount; @amount; end`) to that reader's
/// ivar SymId. Such a call is lowered to an INLINE ivar read on the receiver —
/// no frame, no dispatch (B4, ADR 0034). Sound because the reader is a pure read
/// with no side effects, so a deopt that re-runs the whole method is behaviour-
/// preserving.
pub(crate) fn compile(
    proto: &Proto,
    self_name_id: SymId,
    callees: &FxHashMap<SymId, usize>,
    getters: &FxHashMap<SymId, SymId>,
    syms: &JitSyms,
) -> Option<NativeProto> {
    // Shape gate: exactly one required positional param, nothing fancy.
    if proto.n_required_positional != 1
        || proto.params.len() != 1
        || proto.rest_param.is_some()
        || !proto.kw_param_defaults.is_empty()
    {
        return None;
    }
    let code = &proto.code;
    // Op gate: every op must be one we model. Collect the distinct external
    // callees actually used, so only those get a JIT symbol + import.
    let mut used_callees: Vec<SymId> = Vec::new();
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
            | Op::LoadIvar(_)
            // Key push for a fused `@h[:k]`; standalone use rejected in codegen.
            | Op::LoadSymbol(_)
            | Op::LoadNil => {}
            // All BinOpKinds are modelled in `emit_binop`. Div/Mod emit Ruby
            // floored semantics with a branchless deopt on b==0 and the
            // `i64::MIN / -1` overflow (both fall back to the interpreter).
            Op::BinOp(_) | Op::BinOpLocalLocal(_, _, _) | Op::BinOpInt(_, _) => {}
            // Self-recursive 1-arg call (`fib(n-1)`) → native self-call.
            Op::CallNoRecv(name, 1, _) if *name == self_name_id => {}
            // Call to another already-compiled 1-arg method → native call.
            Op::CallNoRecv(name, 1, _) if callees.contains_key(name) => {
                if !used_callees.contains(name) {
                    used_callees.push(*name);
                }
            }
            // Bare 0-arg call to a self attribute reader (`amount` → `@amount`)
            // → inlined ivar read on the receiver (B4). No callee import needed.
            Op::CallNoRecv(name, 0, _) if getters.contains_key(name) => {}
            // `@arr.length` / `@arr.size` — fused with the preceding LoadIvar in
            // codegen; a standalone one (no LoadIvar before) is rejected there.
            // Any other 0-arg call is admitted here but only LOWERED by codegen
            // when it's the `getter` of a fused `@arr[i].getter` (B4); a
            // standalone one hits codegen's catch-all and declines the proto.
            Op::Call(_, 0, _) => {}
            // `@h[:k]` — `Call([], 1)`, fused with the LoadIvar + LoadSymbol.
            Op::Call(m, 1, _) if *m == syms.bracket => {}
            // `[]` literal + `a << elem` — the array-building shape, where the
            // value-local array is just its Copy `ObjId` (an i64 in codegen).
            Op::NewArray(0) => {}
            Op::Call(m, 1, _) if *m == syms.lshift => {}
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

    let mut builder = JITBuilder::new(cranelift_module::default_libcall_names()).ok()?;
    // Register each used callee's machine address as a symbol the JIT links.
    for cid in &used_callees {
        builder.symbol(format!("c{}", cid.0), callees[cid] as *const u8);
    }
    // Value primitives callable from the body.
    builder.symbol("jit_ivar_get_int", jit_ivar_get_int as *const u8);
    builder.symbol("jit_ivar_len", jit_ivar_len as *const u8);
    builder.symbol("jit_ivar_hash_get_int", jit_ivar_hash_get_int as *const u8);
    builder.symbol("jit_ivar_array_get_int", jit_ivar_array_get_int as *const u8);
    builder.symbol("jit_array_new", jit_array_new as *const u8);
    builder.symbol("jit_array_push", jit_array_push as *const u8);
    builder.symbol("jit_arr_elem_attr_int", jit_arr_elem_attr_int as *const u8);
    let mut module = JITModule::new(builder);
    let ptr_ty = module.target_config().pointer_type();
    let mut ctx = module.make_context();
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(ptr_ty)); // vm
    sig.params.push(AbiParam::new(ptr_ty)); // self (receiver)
    sig.params.push(AbiParam::new(types::I64)); // the i64 arg
    sig.returns.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I8));
    ctx.func.signature = sig.clone();
    let fid = module.declare_function("m", Linkage::Export, &sig).ok()?;
    // The ivar-read primitive: (vm, self, name:i32) -> (i64, i8).
    let mut ivsig = module.make_signature();
    ivsig.params.push(AbiParam::new(ptr_ty));
    ivsig.params.push(AbiParam::new(ptr_ty));
    ivsig.params.push(AbiParam::new(types::I32));
    ivsig.returns.push(AbiParam::new(types::I64));
    ivsig.returns.push(AbiParam::new(types::I8));
    let ivid = module
        .declare_function("jit_ivar_get_int", Linkage::Import, &ivsig)
        .ok()?;
    // `jit_ivar_len` shares the same `(vm, self, name:i32) -> (i64, i8)` sig.
    let alid = module
        .declare_function("jit_ivar_len", Linkage::Import, &ivsig)
        .ok()?;
    // `jit_ivar_hash_get_int`: (vm, self, name:i32, key:i32) -> (i64, i8).
    let mut hgsig = module.make_signature();
    hgsig.params.push(AbiParam::new(ptr_ty));
    hgsig.params.push(AbiParam::new(ptr_ty));
    hgsig.params.push(AbiParam::new(types::I32));
    hgsig.params.push(AbiParam::new(types::I32));
    hgsig.returns.push(AbiParam::new(types::I64));
    hgsig.returns.push(AbiParam::new(types::I8));
    let hgid = module
        .declare_function("jit_ivar_hash_get_int", Linkage::Import, &hgsig)
        .ok()?;
    // `jit_ivar_array_get_int`: (vm, self, name:i32, index:i64) -> (i64, i8).
    let mut agsig = module.make_signature();
    agsig.params.push(AbiParam::new(ptr_ty));
    agsig.params.push(AbiParam::new(ptr_ty));
    agsig.params.push(AbiParam::new(types::I32));
    agsig.params.push(AbiParam::new(types::I64));
    agsig.returns.push(AbiParam::new(types::I64));
    agsig.returns.push(AbiParam::new(types::I8));
    let agid = module
        .declare_function("jit_ivar_array_get_int", Linkage::Import, &agsig)
        .ok()?;
    // `jit_array_new`: (vm) -> i64 (ObjId).
    let mut ansig = module.make_signature();
    ansig.params.push(AbiParam::new(ptr_ty));
    ansig.returns.push(AbiParam::new(types::I64));
    let anid = module
        .declare_function("jit_array_new", Linkage::Import, &ansig)
        .ok()?;
    // `jit_array_push`: (vm, objid:i64, elem:i64) -> void.
    let mut apsig = module.make_signature();
    apsig.params.push(AbiParam::new(ptr_ty));
    apsig.params.push(AbiParam::new(types::I64));
    apsig.params.push(AbiParam::new(types::I64));
    let apid = module
        .declare_function("jit_array_push", Linkage::Import, &apsig)
        .ok()?;
    // `jit_arr_elem_attr_int`: (vm, recv, arr_name:i32, index:i64, getter:i32,
    // cache:ptr) -> (i64, i8).
    let mut aesig = module.make_signature();
    aesig.params.push(AbiParam::new(ptr_ty));
    aesig.params.push(AbiParam::new(ptr_ty));
    aesig.params.push(AbiParam::new(types::I32));
    aesig.params.push(AbiParam::new(types::I64));
    aesig.params.push(AbiParam::new(types::I32));
    aesig.params.push(AbiParam::new(ptr_ty));
    aesig.returns.push(AbiParam::new(types::I64));
    aesig.returns.push(AbiParam::new(types::I8));
    let aeid = module
        .declare_function("jit_arr_elem_attr_int", Linkage::Import, &aesig)
        .ok()?;
    // Each callee imports with the same `(vm, self, i64) -> (i64, i8)` signature.
    let mut callee_fids: FxHashMap<SymId, cranelift_module::FuncId> = FxHashMap::default();
    for cid in &used_callees {
        let cfid = module
            .declare_function(&format!("c{}", cid.0), Linkage::Import, &sig)
            .ok()?;
        callee_fids.insert(*cid, cfid);
    }
    let mut fbctx = FunctionBuilderContext::new();
    // Set inside the codegen block when the returned value is an array ObjId.
    let mut returns_array = false;
    // Inline-cache cells for `@arr[i].getter` sites (B4); their addresses are
    // baked into the code, so they're moved into the NativeProto to outlive it.
    let mut caches: Vec<Box<std::cell::Cell<(usize, u32)>>> = Vec::new();
    {
        let mut fb = FunctionBuilder::new(&mut ctx.func, &mut fbctx);
        // A reference to THIS function, for compiling self-recursive calls
        // (`fib(n-1)` → a native call back into the same code).
        let self_ref = module.declare_func_in_func(fid, fb.func);
        let ivar_ref = module.declare_func_in_func(ivid, fb.func);
        let arraylen_ref = module.declare_func_in_func(alid, fb.func);
        let hashget_ref = module.declare_func_in_func(hgid, fb.func);
        let arrget_ref = module.declare_func_in_func(agid, fb.func);
        let arrnew_ref = module.declare_func_in_func(anid, fb.func);
        let arrpush_ref = module.declare_func_in_func(apid, fb.func);
        let arrelem_ref = module.declare_func_in_func(aeid, fb.func);
        // FuncRefs for each external callee.
        let mut callee_refs: FxHashMap<SymId, FuncRef> = FxHashMap::default();
        for (cid, cfid) in &callee_fids {
            callee_refs.insert(*cid, module.declare_func_in_func(*cfid, fb.func));
        }

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
        // Params: [0]=vm, [1]=self (receiver), [2]=the i64 arg.
        let vm_param = fb.block_params(entry)[0];
        let self_param = fb.block_params(entry)[1];
        let param = fb.block_params(entry)[2];
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
        // Per-local kind — Int by default (a no-op for non-array methods); an
        // array-building local stays ArrayObjId across the loop.
        let mut local_kinds: Vec<Kind> = vec![Kind::Int; nloc];
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
                // Read an ivar via a native primitive (value-touching, AR shape).
                // `@arr.length`/`@arr.size` fuses into one Array-length call;
                // otherwise read an Int ivar. A non-matching heap shape sets ovf
                // → deopt. The fused Call op is skipped (`ip += 1`).
                Op::LoadIvar(s) => {
                    // `@arr[local].getter` (B4) → LoadIvar, LoadLocal, Call([],1),
                    // Call(getter,0). Fused into one inline-cached primitive:
                    // array-get + element-class PIC + int attribute read. The
                    // index local must be Int.
                    let arr_elem_attr = match (code.get(ip + 1), code.get(ip + 2), code.get(ip + 3))
                    {
                        (
                            Some(Op::LoadLocal(slot)),
                            Some(Op::Call(b, 1, _)),
                            Some(Op::Call(g, 0, _)),
                        ) if *b == syms.bracket && local_kinds[*slot as usize] == Kind::Int => {
                            Some((*slot, *g))
                        }
                        _ => None,
                    };
                    if let Some((slot, getter)) = arr_elem_attr {
                        let name = fb.ins().iconst(types::I32, s.0 as i64);
                        let index = fb.use_var(vars[slot as usize]);
                        let gname = fb.ins().iconst(types::I32, getter.0 as i64);
                        let cache = Box::new(std::cell::Cell::new((0usize, 0u32)));
                        let cache_addr =
                            &*cache as *const std::cell::Cell<(usize, u32)> as i64;
                        caches.push(cache);
                        let cache_const = fb.ins().iconst(ptr_ty, cache_addr);
                        let inst = fb.ins().call(
                            arrelem_ref,
                            &[vm_param, self_param, name, index, gname, cache_const],
                        );
                        let (res, of) = {
                            let r = fb.inst_results(inst);
                            (r[0], r[1])
                        };
                        acc_ovf(&mut fb, of);
                        stack.push((res, Kind::Int));
                        ip += 4; // consume LoadIvar + LoadLocal + Call([]) + Call(getter)
                        continue;
                    }
                    // `@h[:k]` → LoadIvar, LoadSymbol(k), Call([], 1).
                    let hash_key = match (code.get(ip + 1), code.get(ip + 2)) {
                        (Some(Op::LoadSymbol(k)), Some(Op::Call(m, 1, _)))
                            if *m == syms.bracket =>
                        {
                            Some(*k)
                        }
                        _ => None,
                    };
                    // `@arr[int]` → LoadIvar, LoadConstInt(idx), Call([], 1).
                    let arr_idx = match (code.get(ip + 1), code.get(ip + 2)) {
                        (Some(Op::LoadConstInt(idx)), Some(Op::Call(m, 1, _)))
                            if *m == syms.bracket =>
                        {
                            Some(*idx)
                        }
                        _ => None,
                    };
                    let name = fb.ins().iconst(types::I32, s.0 as i64);
                    if let Some(k) = hash_key {
                        let key = fb.ins().iconst(types::I32, k.0 as i64);
                        let inst =
                            fb.ins().call(hashget_ref, &[vm_param, self_param, name, key]);
                        let (res, of) = {
                            let r = fb.inst_results(inst);
                            (r[0], r[1])
                        };
                        acc_ovf(&mut fb, of);
                        stack.push((res, Kind::Int));
                        ip += 2; // consume LoadSymbol + Call
                    } else if let Some(idx) = arr_idx {
                        let index = fb.ins().iconst(types::I64, idx);
                        let inst =
                            fb.ins().call(arrget_ref, &[vm_param, self_param, name, index]);
                        let (res, of) = {
                            let r = fb.inst_results(inst);
                            (r[0], r[1])
                        };
                        acc_ovf(&mut fb, of);
                        stack.push((res, Kind::Int));
                        ip += 2; // consume LoadConstInt + Call
                    } else {
                        // `@arr.length`/`.size` → arraylen; else a plain Int ivar.
                        let fuse_len = matches!(
                            code.get(ip + 1),
                            Some(Op::Call(m, 0, _)) if *m == syms.length || *m == syms.size
                        );
                        let prim = if fuse_len { arraylen_ref } else { ivar_ref };
                        let inst = fb.ins().call(prim, &[vm_param, self_param, name]);
                        let (res, of) = {
                            let r = fb.inst_results(inst);
                            (r[0], r[1])
                        };
                        acc_ovf(&mut fb, of);
                        stack.push((res, Kind::Int));
                        if fuse_len {
                            ip += 1;
                        }
                    }
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
                    stack.push((v, local_kinds[*s as usize]));
                }
                Op::StoreLocal(s) => {
                    let (v, k) = stack.pop()?;
                    if k != Kind::Int && k != Kind::ArrayObjId {
                        return None;
                    }
                    local_kinds[*s as usize] = k;
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
                    let (v, k) = stack.pop()?;
                    if k == Kind::ArrayObjId {
                        returns_array = true;
                    }
                    let ov = fb.use_var(ovf_var);
                    fb.ins().return_(&[v, ov]);
                    cur_open = false;
                }
                // `[]` literal → a fresh Array; its `ObjId` lives as an i64.
                Op::NewArray(0) => {
                    let inst = fb.ins().call(arrnew_ref, &[vm_param]);
                    let objid = fb.inst_results(inst)[0];
                    stack.push((objid, Kind::ArrayObjId));
                }
                // `arr << elem` — push the Int onto the array, leave the array
                // (the receiver, since `<<` returns self) on the stack.
                Op::Call(m, 1, _) if *m == syms.lshift => {
                    let (elem, ek) = stack.pop()?;
                    let (arr, ak) = stack.pop()?;
                    if ek != Kind::Int || ak != Kind::ArrayObjId {
                        return None;
                    }
                    fb.ins().call(arrpush_ref, &[vm_param, arr, elem]);
                    stack.push((arr, Kind::ArrayObjId));
                }
                // 0-arg bare call to a self attribute reader (`amount`) → inline
                // the ivar read on the receiver (B4): one native call to
                // `jit_ivar_get_int`, no frame/dispatch. A non-Int ivar deopts.
                Op::CallNoRecv(name, 0, _) if getters.contains_key(name) => {
                    let ivar = getters[name];
                    let nm = fb.ins().iconst(types::I32, ivar.0 as i64);
                    let inst = fb.ins().call(ivar_ref, &[vm_param, self_param, nm]);
                    let (res, of) = {
                        let r = fb.inst_results(inst);
                        (r[0], r[1])
                    };
                    acc_ovf(&mut fb, of);
                    stack.push((res, Kind::Int));
                }
                // 1-arg no-recv call: pop the i64 arg, emit a native call to
                // this function (self-recursion) OR another compiled method,
                // push the result, OR the callee's overflow flag into ours (so a
                // deep overflow deopts the whole tree).
                Op::CallNoRecv(name, 1, _)
                    if *name == self_name_id || callee_refs.contains_key(name) =>
                {
                    let fref = if *name == self_name_id {
                        self_ref
                    } else {
                        callee_refs[name]
                    };
                    let (arg, ka) = stack.pop()?;
                    if ka != Kind::Int {
                        return None;
                    }
                    // Forward vm + self so the callee can touch the heap too.
                    let inst = fb.ins().call(fref, &[vm_param, self_param, arg]);
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
    let ptr = unsafe {
        std::mem::transmute::<_, extern "C" fn(*const crate::vm::Vm, *const Value, i64) -> NRet>(
            code_ptr,
        )
    };
    Some(NativeProto {
        _module: module,
        ptr,
        guard_class: std::cell::Cell::new(0),
        returns_array: std::cell::Cell::new(returns_array),
        _caches: caches,
    })
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
    // Div/Mod: Ruby floored division (remainder takes the divisor's sign),
    // mirroring `floor_div_i64`/`floor_mod_i64`. Deopt (ovf=1) on `b == 0` and
    // the lone overflow case `i64::MIN / -1`; the divisor is guarded to 1 in
    // those cases so Cranelift's trapping `sdiv`/`srem` never fire (the result
    // is discarded by the caller on deopt). Branchless via `select`.
    if matches!(k, BinOpKind::Div | BinOpKind::Mod) {
        let zero = fb.ins().iconst(types::I64, 0);
        let neg1 = fb.ins().iconst(types::I64, -1);
        let one = fb.ins().iconst(types::I64, 1);
        let min = fb.ins().iconst(types::I64, i64::MIN);
        let is_zero = fb.ins().icmp(IntCC::Equal, b, zero);
        let a_min = fb.ins().icmp(IntCC::Equal, a, min);
        let b_neg1 = fb.ins().icmp(IntCC::Equal, b, neg1);
        let ovf_case = fb.ins().band(a_min, b_neg1);
        let deopt = fb.ins().bor(is_zero, ovf_case);
        let safe_b = fb.ins().select(deopt, one, b);
        let q = fb.ins().sdiv(a, safe_b);
        let r = fb.ins().srem(a, safe_b);
        // need_adj = (r != 0) && ((r ^ safe_b) < 0)  — signs of r and b differ.
        let r_ne0 = fb.ins().icmp(IntCC::NotEqual, r, zero);
        let rxorb = fb.ins().bxor(r, safe_b);
        let signs_differ = fb.ins().icmp(IntCC::SignedLessThan, rxorb, zero);
        let need_adj = fb.ins().band(r_ne0, signs_differ);
        let q_adj = fb.ins().iadd_imm(q, -1);
        let r_adj = fb.ins().iadd(r, safe_b);
        let q_final = fb.ins().select(need_adj, q_adj, q);
        let r_final = fb.ins().select(need_adj, r_adj, r);
        let res = if matches!(k, BinOpKind::Div) { q_final } else { r_final };
        let cur = fb.use_var(ovf_var);
        let nv = fb.ins().bor(cur, deopt);
        fb.def_var(ovf_var, nv);
        stack.push((res, Kind::Int));
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

/// A compiled value-method: `fn(vm, recv, arg0, out)` — takes the receiver and
/// the first argument as `Value` pointers and writes the result `Value` to
/// `out`. Holds the `JITModule`. (`arg0` is `&Nil` for 0-arg shapes.)
pub(crate) struct ValueProto {
    _module: JITModule,
    ptr: extern "C" fn(*const crate::vm::Vm, *const Value, *const Value, *mut Value),
}

impl ValueProto {
    /// Run the native value-method. `vm` is borrowed shared for the duration of
    /// the call (the primitive only reads the heap).
    #[inline]
    pub(crate) fn call(&self, vm: *const crate::vm::Vm, recv: &Value, arg0: &Value) -> Value {
        let mut out = Value::Nil;
        (self.ptr)(
            vm,
            recv as *const Value,
            arg0 as *const Value,
            &mut out as *mut Value,
        );
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

/// Native primitive for the INTEGER JIT: read `recv`'s ivar `name` and return it
/// as i64 if it's an `Int`, else signal deopt (`ovf = 1`). This is the seam by
/// which an integer-typed native loop reads an Integer attribute — the AR
/// aggregation shape (`while …; total += @amount; …`).
///
/// # Safety
/// `vm`, `recv` must be valid for the call.
pub(crate) unsafe extern "C" fn jit_ivar_get_int(
    vm: *const crate::vm::Vm,
    recv: *const Value,
    name: u32,
) -> NRet {
    let vm = unsafe { &*vm };
    let recv = unsafe { &*recv };
    let name_id = crate::intern::SymId(name);
    // Match the ivar by REFERENCE and copy out the i64 — no `Value` clone (this
    // is hot: a method may read several ivars per loop iteration).
    let n = match recv {
        Value::Object(oid) => match vm.heap.get(*oid) {
            crate::heap::HeapObj::Instance(inst) => match inst.ivars.get(&name_id) {
                Some(Value::Int(n)) => Some(*n),
                _ => None,
            },
            _ => None,
        },
        Value::Class(cls) => match cls.ivars.borrow().get(&name_id) {
            Some(Value::Int(n)) => Some(*n),
            _ => None,
        },
        _ => None,
    };
    match n {
        Some(n) => NRet { res: n, ovf: 0 },
        None => NRet { res: 0, ovf: 1 }, // missing or non-Int → deopt to the interpreter
    }
}

/// Method-name syms the JIT recognises for value-primitive fusion (e.g. fusing
/// `@items.size` / `@h[:k]` into one native call). Built once by the hook.
pub(crate) struct JitSyms {
    pub length: SymId,
    pub size: SymId,
    pub bracket: SymId,
    pub lshift: SymId,
}

/// Native primitive: allocate a fresh empty Array, return its `ObjId` as i64.
/// Does NOT call `maybe_gc`, so no collection runs mid-method — the array is
/// rooted by the caller's stack on return, and nothing in between can collect.
///
/// # Safety
/// `vm` must be valid; the call mutates the heap.
pub(crate) unsafe extern "C" fn jit_array_new(vm: *mut crate::vm::Vm) -> i64 {
    let vm = unsafe { &mut *vm };
    let id = vm
        .heap
        .alloc(crate::heap::HeapObj::Array(Vec::<Value>::new().into()));
    id.0 as i64
}

/// Native primitive: push `Value::Int(elem)` onto the Array `objid`.
///
/// # Safety
/// `vm` valid; `objid` a live Array slot (produced by `jit_array_new` earlier in
/// the same method, never collected mid-method).
pub(crate) unsafe extern "C" fn jit_array_push(vm: *mut crate::vm::Vm, objid: i64, elem: i64) {
    let vm = unsafe { &mut *vm };
    let id = crate::value::ObjId(objid as u32);
    vm.heap.array_mut(id).push(Value::Int(elem));
}

/// Native primitive: `recv.@name[:key]` where the ivar is a Hash with a Symbol
/// key whose value is an Int — returns it as i64, else deopt. The AR
/// `@attributes[:col]` shape (an integer attribute read in a loop).
///
/// # Safety
/// `vm`, `recv` must be valid for the call.
pub(crate) unsafe extern "C" fn jit_ivar_hash_get_int(
    vm: *const crate::vm::Vm,
    recv: *const Value,
    name: u32,
    key: u32,
) -> NRet {
    let vm = unsafe { &*vm };
    let recv = unsafe { &*recv };
    let name_id = crate::intern::SymId(name);
    let v = match recv {
        Value::Object(oid) => match vm.heap.get(*oid) {
            crate::heap::HeapObj::Instance(inst) => inst.ivars.get(&name_id).cloned(),
            _ => None,
        },
        Value::Class(cls) => cls.ivars.borrow().get(&name_id).cloned(),
        _ => None,
    };
    match v {
        Some(Value::Hash(hid)) => {
            let want = Value::Sym(crate::intern::SymId(key));
            for (hk, hv) in vm.heap.hash(hid) {
                if hk.ruby_eql(&want, &vm.heap) {
                    return match hv {
                        Value::Int(n) => NRet { res: *n, ovf: 0 },
                        _ => NRet { res: 0, ovf: 1 }, // non-Int value → deopt
                    };
                }
            }
            NRet { res: 0, ovf: 1 } // key absent (CRuby nil) → deopt
        }
        _ => NRet { res: 0, ovf: 1 },
    }
}

/// Native primitive: `recv.@name[index]` where the ivar is an Array and the
/// element is an Int — returns it as i64, else deopt. The AR result-row
/// column-read shape (`row[2]` over a query result). Ruby negative indices
/// supported; out-of-bounds (CRuby nil) or non-Int element → deopt.
///
/// # Safety
/// `vm`, `recv` must be valid for the call.
pub(crate) unsafe extern "C" fn jit_ivar_array_get_int(
    vm: *const crate::vm::Vm,
    recv: *const Value,
    name: u32,
    index: i64,
) -> NRet {
    let vm = unsafe { &*vm };
    let recv = unsafe { &*recv };
    let name_id = crate::intern::SymId(name);
    let read = |iv: Option<&Value>| -> Option<i64> {
        match iv {
            Some(Value::Array(aid)) => {
                let arr = vm.heap.array(*aid);
                let i = if index < 0 {
                    arr.len() as i64 + index
                } else {
                    index
                };
                if i >= 0 && (i as usize) < arr.len() {
                    match &arr[i as usize] {
                        Value::Int(n) => Some(*n),
                        _ => None, // non-Int element → deopt
                    }
                } else {
                    None // out of bounds (CRuby nil) → deopt
                }
            }
            _ => None,
        }
    };
    let res = match recv {
        Value::Object(oid) => match vm.heap.get(*oid) {
            crate::heap::HeapObj::Instance(inst) => read(inst.ivars.get(&name_id)),
            _ => None,
        },
        Value::Class(cls) => read(cls.ivars.borrow().get(&name_id)),
        _ => None,
    };
    match res {
        Some(n) => NRet { res: n, ovf: 0 },
        None => NRet { res: 0, ovf: 1 },
    }
}

/// Native primitive (B4): `recv.@arr[index].getter`, where `getter` is a simple
/// int attribute reader on the element. The AR aggregation shape — iterate a
/// collection ivar and read an integer attribute off each element.
///
/// A MONOMORPHIC inline cache `cache` holds `(element_class_ptr, ivar_sym)`. On
/// a class hit it reads the cached ivar; on an empty cache it fills it (resolve
/// `getter` → `getter_ivar` on the element's class); on a class MISS (a
/// different element class — megamorphic) it deopts, so the interpreter runs
/// that iteration. Also deopts (ovf=1) on: a non-Array ivar, an out-of-bounds or
/// non-Object element, a `getter` that isn't a simple reader, or a non-Int
/// attribute. Every deopt re-runs the whole (pure) driver in the interpreter, so
/// behaviour is preserved.
///
/// # Safety
/// `vm`, `recv`, `cache` must be valid for the call.
pub(crate) unsafe extern "C" fn jit_arr_elem_attr_int(
    vm: *const crate::vm::Vm,
    recv: *const Value,
    arr_name: u32,
    index: i64,
    getter_name: u32,
    cache: *const std::cell::Cell<(usize, u32)>,
) -> NRet {
    let deopt = NRet { res: 0, ovf: 1 };
    let vm = unsafe { &*vm };
    let recv = unsafe { &*recv };
    let cache = unsafe { &*cache };
    let arr_name_id = crate::intern::SymId(arr_name);
    // recv.@arr — must be an Array ivar.
    let arr_id = {
        let iv = match recv {
            Value::Object(oid) => match vm.heap.get(*oid) {
                crate::heap::HeapObj::Instance(inst) => inst.ivars.get(&arr_name_id).cloned(),
                _ => None,
            },
            Value::Class(cls) => cls.ivars.borrow().get(&arr_name_id).cloned(),
            _ => None,
        };
        match iv {
            Some(Value::Array(aid)) => aid,
            _ => return deopt,
        }
    };
    // element at index (Ruby negative wrap; out of bounds → deopt).
    let elem_oid = {
        let arr = vm.heap.array(arr_id);
        let i = if index < 0 { arr.len() as i64 + index } else { index };
        if i < 0 || i as usize >= arr.len() {
            return deopt;
        }
        match &arr[i as usize] {
            Value::Object(eoid) => *eoid,
            _ => return deopt,
        }
    };
    let ecls = match vm.heap.try_class_of(elem_oid) {
        Some(c) => c,
        None => return deopt,
    };
    let ecls_ptr = std::rc::Rc::as_ptr(&ecls) as usize;
    // Inline cache: hit → cached ivar; empty → fill; class miss → deopt.
    let (cls_ptr, cached_ivar) = cache.get();
    let ivar = if cls_ptr == ecls_ptr {
        cached_ivar
    } else if cls_ptr == 0 {
        let m = match vm.lookup_method_uncached(&ecls, crate::intern::SymId(getter_name)) {
            Some(m) => m,
            None => return deopt,
        };
        let iv = match vm.protos[m.proto_idx].getter_ivar {
            Some(iv) => iv,
            None => return deopt, // not a simple reader → interpreter
        };
        cache.set((ecls_ptr, iv.0));
        iv.0
    } else {
        return deopt; // megamorphic site → interpreter
    };
    // element.@ivar — must be an Int.
    match vm.heap.get(elem_oid) {
        crate::heap::HeapObj::Instance(inst) => {
            match inst.ivars.get(&crate::intern::SymId(ivar)) {
                Some(Value::Int(n)) => NRet { res: *n, ovf: 0 },
                _ => deopt,
            }
        }
        _ => deopt,
    }
}

/// Native primitive: `recv.@name.length` / `.size` where the ivar is an Array
/// (element count) or a String (character count) — returns it as i64, else
/// deopt. The AR `has_many` collection-size and string-attribute-length shapes.
/// Non-default (registry) string encodings deopt so the interpreter's
/// encoding-aware count stays authoritative.
///
/// # Safety
/// `vm`, `recv` must be valid for the call.
pub(crate) unsafe extern "C" fn jit_ivar_len(
    vm: *const crate::vm::Vm,
    recv: *const Value,
    name: u32,
) -> NRet {
    let vm = unsafe { &*vm };
    let recv = unsafe { &*recv };
    let name_id = crate::intern::SymId(name);
    // Compute the length by REFERENCE — no `Value` clone (a String clone would
    // bump the Rc every iteration).
    let len = |iv: Option<&Value>| -> Option<i64> {
        match iv {
            Some(Value::Array(aid)) => Some(vm.heap.array(*aid).len() as i64),
            Some(Value::Str(rs)) => match rs.encoding.get() {
                crate::value::EncodingTag::Other(_) => None, // registry enc → interp
                _ => Some(rs.char_count() as i64),
            },
            _ => None,
        }
    };
    let res = match recv {
        Value::Object(oid) => match vm.heap.get(*oid) {
            crate::heap::HeapObj::Instance(inst) => len(inst.ivars.get(&name_id)),
            _ => None,
        },
        Value::Class(cls) => len(cls.ivars.borrow().get(&name_id)),
        _ => None,
    };
    match res {
        Some(n) => NRet { res: n, ovf: 0 },
        None => NRet { res: 0, ovf: 1 }, // not an Array/String → deopt
    }
}

/// Compile a value-method whose body is a single ivar read, to a native call
/// into `jit_ivar_get`. Two recognised shapes (else `None`):
///   - getter:        `[LoadIvar(s), Return]`                       → read recv's `@s`
///   - ivar_get wrap: `[LoadLocal(0), LoadSymbol(s), Call(ivg,1), Return]`
///                                                                  → read arg0's `@s`
/// The latter is the AR-shaped, NON-fast-pathed win: the interpreter pays a full
/// `instance_variable_get` dispatch + a frame; native code pays one direct call.
/// Native primitive: `recv.@name[key]` returning the Hash value (ANY type) by
/// pointer — the AR `record[:col]` attribute reader (a column's actual value, a
/// String/Int/etc.), not just an Int. Absent key or non-Hash ivar → nil.
/// Reads the heap and copies an EXISTING value out (no new allocation).
///
/// # Safety
/// `vm`, `recv`, `key`, `out` must be valid for the call.
pub(crate) unsafe extern "C" fn jit_hash_get_value(
    vm: *const crate::vm::Vm,
    recv: *const Value,
    name: u32,
    key: *const Value,
    out: *mut Value,
) {
    let vm = unsafe { &*vm };
    let recv = unsafe { &*recv };
    let key = unsafe { &*key };
    let name_id = crate::intern::SymId(name);
    let result = match recv {
        Value::Object(oid) => match vm.heap.get(*oid) {
            crate::heap::HeapObj::Instance(inst) => match inst.ivars.get(&name_id) {
                Some(Value::Hash(hid)) => {
                    let hid = *hid;
                    vm.heap
                        .hash(hid)
                        .iter()
                        .find(|(hk, _)| hk.ruby_eql(key, &vm.heap))
                        .map(|(_, v)| v.clone())
                }
                _ => None,
            },
            _ => None,
        },
        _ => None,
    };
    unsafe { std::ptr::write(out, result.unwrap_or(Value::Nil)) };
}

/// Recognised value-method body shapes.
enum ValuePat {
    /// `jit_ivar_get(target, sym)` — getter (target=recv) or ivar_get wrapper
    /// (target=arg0).
    IvarGet { sym: crate::intern::SymId, read_arg0: bool },
    /// `jit_hash_get_value(recv, sym, arg0)` — `@h[key]` returning any Value.
    HashAttr { sym: crate::intern::SymId },
}

pub(crate) fn compile_value(
    proto: &Proto,
    ivg_sym: crate::intern::SymId,
    bracket_sym: crate::intern::SymId,
) -> Option<ValueProto> {
    let pat = match proto.code.as_slice() {
        [Op::LoadIvar(s), Op::Return] => ValuePat::IvarGet { sym: *s, read_arg0: false },
        [Op::LoadLocal(0), Op::LoadSymbol(s), Op::Call(name, 1, _), Op::Return]
            if *name == ivg_sym =>
        {
            ValuePat::IvarGet { sym: *s, read_arg0: true }
        }
        // `def attr(k); @attributes[k]; end` → @ivar[arg0] returning any Value.
        [Op::LoadIvar(s), Op::LoadLocal(0), Op::Call(name, 1, _), Op::Return]
            if *name == bracket_sym =>
        {
            ValuePat::HashAttr { sym: *s }
        }
        _ => return None,
    };

    let mut builder = JITBuilder::new(cranelift_module::default_libcall_names()).ok()?;
    builder.symbol("jit_ivar_get", jit_ivar_get as *const u8);
    builder.symbol("jit_hash_get_value", jit_hash_get_value as *const u8);
    let mut module = JITModule::new(builder);
    let ptr_ty = module.target_config().pointer_type();
    let mut ctx = module.make_context();
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(ptr_ty)); // vm
    sig.params.push(AbiParam::new(ptr_ty)); // recv
    sig.params.push(AbiParam::new(ptr_ty)); // arg0
    sig.params.push(AbiParam::new(ptr_ty)); // out
    ctx.func.signature = sig.clone();
    let fid = module.declare_function("g", Linkage::Export, &sig).ok()?;
    // jit_ivar_get: (vm, target, name:i32, out)
    let mut igsig = module.make_signature();
    igsig.params.push(AbiParam::new(ptr_ty));
    igsig.params.push(AbiParam::new(ptr_ty));
    igsig.params.push(AbiParam::new(types::I32));
    igsig.params.push(AbiParam::new(ptr_ty));
    let igid = module.declare_function("jit_ivar_get", Linkage::Import, &igsig).ok()?;
    // jit_hash_get_value: (vm, recv, name:i32, key:ptr, out)
    let mut hgsig = module.make_signature();
    hgsig.params.push(AbiParam::new(ptr_ty));
    hgsig.params.push(AbiParam::new(ptr_ty));
    hgsig.params.push(AbiParam::new(types::I32));
    hgsig.params.push(AbiParam::new(ptr_ty));
    hgsig.params.push(AbiParam::new(ptr_ty));
    let hgid = module
        .declare_function("jit_hash_get_value", Linkage::Import, &hgsig)
        .ok()?;
    let mut fbctx = FunctionBuilderContext::new();
    {
        let mut fb = FunctionBuilder::new(&mut ctx.func, &mut fbctx);
        let igref = module.declare_func_in_func(igid, fb.func);
        let hgref = module.declare_func_in_func(hgid, fb.func);
        let b = fb.create_block();
        fb.append_block_params_for_function_params(b);
        fb.switch_to_block(b);
        fb.seal_block(b);
        let vm = fb.block_params(b)[0];
        let recv = fb.block_params(b)[1];
        let arg0 = fb.block_params(b)[2];
        let out = fb.block_params(b)[3];
        match pat {
            ValuePat::IvarGet { sym, read_arg0 } => {
                let target = if read_arg0 { arg0 } else { recv };
                let name = fb.ins().iconst(types::I32, sym.0 as i64);
                fb.ins().call(igref, &[vm, target, name, out]);
            }
            ValuePat::HashAttr { sym } => {
                let name = fb.ins().iconst(types::I32, sym.0 as i64);
                fb.ins().call(hgref, &[vm, recv, name, arg0, out]);
            }
        }
        fb.ins().return_(&[]);
        fb.finalize();
    }
    module.define_function(fid, &mut ctx).ok()?;
    module.clear_context(&mut ctx);
    module.finalize_definitions().ok()?;
    let code = module.get_finalized_function(fid);
    let ptr = unsafe {
        std::mem::transmute::<
            _,
            extern "C" fn(*const crate::vm::Vm, *const Value, *const Value, *mut Value),
        >(code)
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
