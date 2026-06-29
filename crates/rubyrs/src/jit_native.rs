//! End-to-end native (Cranelift) JIT for 1-parameter integer methods.
//! ADR 0030 finding #4: lower a real `Proto` to machine code so a Ruby
//! method call dispatches into native code, with an overflow-guard deopt.
//!
//! Eligibility (else `None`, stays interpreted): exactly one required
//! positional param, no rest/kw, and every op in a small integer set
//! (const/local load+store, +/-/* and comparisons, jumps, return). Any
//! arithmetic overflow OR an arg that isn't an `Int` deopts to the
//! interpreter — so the JIT can never change a result, only its speed.

use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{
    types, AbiParam, Block, BlockArg, FuncRef, InstBuilder, MemFlagsData, Value as ClValue,
};
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
    /// When true the i64 result is f64 BITS — the dispatch boxes it to
    /// `Value::Float`. Set for a METHOD whose body produces a Float (e.g.
    /// `def scale(n); n * 1.5; end`); a method that mixes Float and non-Float
    /// returns declines to compile (the box kind would be ambiguous).
    pub(crate) returns_float: std::cell::Cell<bool>,
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
    /// A Cranelift `F64` value (an actual float, not its bits) on the operand
    /// stack. Float LOCALS still live in `I64` vars holding the f64 BITS — a
    /// `LoadLocal` of a Float local bitcasts I64->F64, a `StoreLocal`/`Return`
    /// bitcasts F64->I64 — so the whole ABI stays uniformly i64 and the Int paths
    /// are untouched. Only pure Float<->Float arithmetic is modelled (mixed
    /// Int/Float declines: no coercion yet).
    Float,
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
/// `block` is `Some((param_slot, body_local_start))` when compiling a 1-param
/// BLOCK rather than a method (B5): the single arg binds to `param_slot` (not
/// local 0) in the block's shared-locals layout, and any access to a captured
/// OUTER slot (`< body_local_start`, other than the param) declines — so only a
/// pure function of the param + the block's own temporaries compiles. `None` =
/// a normal method (param at slot 0, the method shape-gate applies).
/// How a block threads an accumulator (the 4th element of `compile`'s `block`
/// spec). The C ABI gains a 2nd i64 arg for the two accumulator kinds.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AccKind {
    /// 1-param block: `(vm, self, elem)`. The sole param binds the element.
    None,
    /// `inject`/`reduce` `|acc, x|`: `(vm, self, acc, elem)`. The accumulator is
    /// the 1st block param (`param_slot`), the element the 2nd; the block's
    /// RETURN value is the new accumulator.
    Inject,
    /// `each` accumulator `{ |x| total += x }`: `(vm, self, acc, elem)`. The
    /// element is the sole param (`param_slot`); the accumulator is a CAPTURED
    /// outer slot. The new accumulator is that slot's value AFTER the body (the
    /// block's own return is discarded by `each`), so a body that stores the acc
    /// then returns something else can't corrupt it.
    EachAcc { acc_slot: u32 },
    /// `each_with_object(coll) { |elem, memo| memo << f(elem) }`:
    /// `(vm, self, elem, memo)`. The element is the 1st param (`param_slot`), the
    /// `memo` collection the 2nd — bound to a SCRATCH array (kind `ArrayObjId`)
    /// the block pushes into via `<<`. The block's return is discarded; the loop
    /// returns the scratch ObjId.
    EachObj,
    /// `each_with_index { |x, i| total += f(x, i) }`: `(vm, self, acc, elem, idx)`.
    /// Like `EachAcc` but the block has a 2nd param — the iteration index — bound
    /// to a 3rd i64 C-arg. Element is `param_slot`, index `param_slot + 1`, the
    /// accumulator a CAPTURED slot; the new acc is that slot's value after the body.
    EachWithIndex { acc_slot: u32 },
}

pub(crate) fn compile(
    proto: &Proto,
    self_name_id: SymId,
    callees: &FxHashMap<SymId, usize>,
    // FLOAT cross-call addresses: a callee's fparam (float-param) machine address,
    // used when a `CallNoRecv(name, 1)` in this body passes a Float arg. The callee
    // takes f64 bits (i64 ABI) and returns f64 bits — the caller bitcasts at the
    // boundary. Empty for blocks + leaf methods. This is what lets a compiled method
    // INLINE a float-arg call (`run` -> `poly(k*0.5)`) instead of paying dispatch.
    float_callees: &FxHashMap<SymId, usize>,
    getters: &FxHashMap<SymId, SymId>,
    syms: &JitSyms,
    block: Option<(u32, u32, bool, AccKind)>,
    // The iteration element is a Float (its i64 C-arg holds the f64 bits). Only
    // valid for a 1-param value block (Float-element drivers like `sum`); the
    // param binds as `Kind::Float`. `false` for every Int driver + all methods.
    float_elem: bool,
    // The accumulator / value-result is a Float (independent of the element kind).
    // Decoupled from `float_elem` so an INT-element block whose body produces a
    // Float (`ints.sum { |x| x * 1.5 }`) can return f64 bits into a Float
    // accumulator. Existing callers pass `float_acc == float_elem` (behaviour-
    // identical); only the Int-elem/Float-acc sum driver passes (false, true).
    float_acc: bool,
) -> Option<NativeProto> {
    // Shape gate (methods only): exactly one required positional param. A block's
    // 1-param eligibility is checked by the caller via its `BlockHandle` fields.
    if block.is_none()
        && (proto.n_required_positional != 1
            || proto.params.len() != 1
            || proto.rest_param.is_some()
            || !proto.kw_param_defaults.is_empty())
    {
        return None;
    }
    let param_slot = block.map(|(p, _, _, _)| p).unwrap_or(0);
    // Predicate mode (count/select/...): a final `Bool` Return materialises as
    // i64 0/1 instead of declining. Only meaningful for blocks; methods (None)
    // are never predicate.
    let predicate = block.map(|(_, _, p, _)| p).unwrap_or(false);
    // Accumulator layout (see `AccKind`). `two_param` adds a 2nd i64 C-arg;
    // `acc_slot`/`elem_slot` say where the accumulator and the iteration element
    // bind — they differ between `inject` (acc is the 1st block param) and an
    // `each`-accumulator (acc is a CAPTURED slot, element is the only param).
    let acc_kind = block.map(|(_, _, _, a)| a).unwrap_or(AccKind::None);
    // Which local slot each C-ABI arg binds to. `arg2` is the 3rd C param, `arg3`
    // (2-param only) the 4th — the driver loop decides what it passes there:
    //   Inject    blk(vm,self,acc,elem)     -> arg2=acc(slot), arg3=elem(slot+1)
    //   EachAcc   blk(vm,self,acc,elem)     -> arg2=captured acc, arg3=elem(slot)
    //   EachObj   blk(vm,self,elem,memo)    -> arg2=elem(slot), arg3=memo(slot+1)
    //   EachIndex blk(vm,self,acc,elem,idx) -> arg2=captured acc, arg3=elem(slot),
    //                                          arg4=index(slot+1)
    let (arg2_slot, arg3_slot, arg4_slot): (u32, Option<u32>, Option<u32>) = match acc_kind {
        AccKind::None => (param_slot, None, None),
        AccKind::Inject => (param_slot, Some(param_slot + 1), None),
        AccKind::EachAcc { acc_slot } => (acc_slot, Some(param_slot), None),
        AccKind::EachObj => (param_slot, Some(param_slot + 1), None),
        AccKind::EachWithIndex { acc_slot } => {
            (acc_slot, Some(param_slot), Some(param_slot + 1))
        }
    };
    let two_param = arg3_slot.is_some();
    let three_param = arg4_slot.is_some();
    // each-accumulator / each_with_index: `Return` yields the captured accumulator
    // (arg2_slot)'s value, not the block's (discarded) return.
    let acc_from_slot = matches!(
        acc_kind,
        AccKind::EachAcc { .. } | AccKind::EachWithIndex { .. }
    );
    // each_with_object: the `memo` arg (arg3_slot) is an Array the block pushes to
    // via `<<`, so it carries the `ArrayObjId` kind, not `Int`.
    let memo_slot = match acc_kind {
        AccKind::EachObj => arg3_slot,
        _ => None,
    };
    let is_param = |s: u32| s == arg2_slot || arg3_slot == Some(s) || arg4_slot == Some(s);
    // For a block, reject reads/writes of captured outer slots (closure state):
    // a slot below the body-local start that isn't a param. Methods (None)
    // impose no such restriction.
    if let Some((_, body_start, _, _)) = block {
        for op in &proto.code {
            let slot = match op {
                Op::LoadLocal(s) | Op::StoreLocal(s) | Op::IncLocal(s) | Op::IncLocalNoPush(s) => {
                    Some(*s)
                }
                Op::BinOpLocalLocal(_, a, b) => {
                    if ((*a as u32) < body_start && !is_param(*a as u32))
                        || ((*b as u32) < body_start && !is_param(*b as u32))
                    {
                        return None;
                    }
                    None
                }
                _ => None,
            };
            if let Some(s) = slot
                && (s as u32) < body_start
                && !is_param(s as u32)
            {
                return None;
            }
        }
    }
    let code = &proto.code;
    // Op gate: every op must be one we model. Collect the distinct external
    // callees actually used, so only those get a JIT symbol + import.
    let mut used_callees: Vec<SymId> = Vec::new();
    let mut used_float_callees: Vec<SymId> = Vec::new();
    for op in code {
        match op {
            Op::LoadConstInt(_)
            | Op::LoadConstFloat(_)
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
            // Call to another already-compiled 1-arg method → native call. A name
            // may be in BOTH maps (called with an Int arg at one site, Float at
            // another); register whichever symbols the maps offer — the codegen
            // picks the right one by the arg's kind at each site.
            Op::CallNoRecv(name, 1, _)
                if callees.contains_key(name) || float_callees.contains_key(name) =>
            {
                if callees.contains_key(name) && !used_callees.contains(name) {
                    used_callees.push(*name);
                }
                if float_callees.contains_key(name) && !used_float_callees.contains(name) {
                    used_float_callees.push(*name);
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
            // The fused `x.method` op — admitted only for the pure unary Int
            // primitives codegen models (`x.abs`/`x.even?`/…); any other
            // `LoadLocalCall` hits the catch-all below and declines.
            Op::LoadLocalCall(_, name, _)
                if is_int_unary(*name, syms) || is_float_to_int(*name, syms) => {}
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
    // Float cross-call callees get a distinct `cf{cid}` symbol (the fparam address).
    for cid in &used_float_callees {
        builder.symbol(format!("cf{}", cid.0), float_callees[cid] as *const u8);
    }
    // Value primitives callable from the body.
    builder.symbol("jit_ivar_get_int", jit_ivar_get_int as *const u8);
    builder.symbol("jit_ivar_len", jit_ivar_len as *const u8);
    builder.symbol("jit_ivar_hash_get_int", jit_ivar_hash_get_int as *const u8);
    builder.symbol("jit_ivar_array_get_int", jit_ivar_array_get_int as *const u8);
    builder.symbol("jit_array_new", jit_array_new as *const u8);
    builder.symbol("jit_array_push", jit_array_push as *const u8);
    builder.symbol("jit_array_push_float", jit_array_push_float as *const u8);
    builder.symbol("jit_arr_elem_attr_int", jit_arr_elem_attr_int as *const u8);
    let mut module = JITModule::new(builder);
    let ptr_ty = module.target_config().pointer_type();
    let mut ctx = module.make_context();
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(ptr_ty)); // vm
    sig.params.push(AbiParam::new(ptr_ty)); // self (receiver)
    sig.params.push(AbiParam::new(types::I64)); // the i64 arg (acc, for a 2-param block)
    if two_param {
        sig.params.push(AbiParam::new(types::I64)); // 2nd arg (the element)
    }
    if three_param {
        sig.params.push(AbiParam::new(types::I64)); // 3rd arg (each_with_index's index)
    }
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
    // `jit_array_push_float`: (vm, objid:i64, bits:i64) -> void (Float `memo << f(x)`).
    let mut apfsig = module.make_signature();
    apfsig.params.push(AbiParam::new(ptr_ty));
    apfsig.params.push(AbiParam::new(types::I64));
    apfsig.params.push(AbiParam::new(types::I64));
    let apfid = module
        .declare_function("jit_array_push_float", Linkage::Import, &apfsig)
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
    // Float cross-call callees share the same `(vm, self, i64) -> (i64, i8)` ABI —
    // only the i64s' meaning (f64 bits) differs, handled at the call boundary.
    let mut float_callee_fids: FxHashMap<SymId, cranelift_module::FuncId> = FxHashMap::default();
    for cid in &used_float_callees {
        let cfid = module
            .declare_function(&format!("cf{}", cid.0), Linkage::Import, &sig)
            .ok()?;
        float_callee_fids.insert(*cid, cfid);
    }
    let mut fbctx = FunctionBuilderContext::new();
    // Set inside the codegen block when the returned value is an array ObjId.
    let mut returns_array = false;
    // METHOD (block.is_none()) Float-return tracking: a method may produce a Float
    // result (`def scale(n); n*1.5; end`), boxed Value::Float by dispatch. Mixing
    // Float and non-Float (Int/Array) value returns makes the box kind ambiguous —
    // decline. (Block drivers can't mix: the Int/Float Return rules already force a
    // single kind per driver.)
    let is_method = block.is_none();
    let mut m_float_ret = false;
    let mut m_nonfloat_ret = false;
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
        let arrpushf_ref = module.declare_func_in_func(apfid, fb.func);
        let arrelem_ref = module.declare_func_in_func(aeid, fb.func);
        // FuncRefs for each external callee.
        let mut callee_refs: FxHashMap<SymId, FuncRef> = FxHashMap::default();
        for (cid, cfid) in &callee_fids {
            callee_refs.insert(*cid, module.declare_func_in_func(*cfid, fb.func));
        }
        let mut float_callee_refs: FxHashMap<SymId, FuncRef> = FxHashMap::default();
        for (cid, cfid) in &float_callee_fids {
            float_callee_refs.insert(*cid, module.declare_func_in_func(*cfid, fb.func));
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
        // Params: [0]=vm, [1]=self (receiver), [2]=the i64 arg, and for a 2-param
        // block (inject) [3]=the 2nd arg (element).
        let vm_param = fb.block_params(entry)[0];
        let self_param = fb.block_params(entry)[1];
        let param = fb.block_params(entry)[2];
        let param2 = if two_param {
            Some(fb.block_params(entry)[3])
        } else {
            None
        };
        let param3 = if three_param {
            Some(fb.block_params(entry)[4])
        } else {
            None
        };
        let nloc = proto.n_locals as usize;
        let vars: Vec<Variable> = (0..nloc).map(|_| fb.declare_var(types::I64)).collect();
        // Bind the C args to their slots: arg[2] (`param`) -> `arg2_slot`, arg[3]
        // (`param2`) -> `arg3_slot`, arg[4] (`param3`) -> `arg4_slot`. Else 0.
        for (i, v) in vars.iter().enumerate() {
            let iu = i as u32;
            if iu == arg2_slot {
                fb.def_var(*v, param);
            } else if arg3_slot == Some(iu) {
                fb.def_var(*v, param2.unwrap());
            } else if arg4_slot == Some(iu) {
                fb.def_var(*v, param3.unwrap());
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
        // array-building local stays ArrayObjId across the loop. each_with_object's
        // `memo` param is an Array (the scratch) the block pushes to.
        let mut local_kinds: Vec<Kind> = vec![Kind::Int; nloc];
        if let Some(m) = memo_slot {
            local_kinds[m as usize] = Kind::ArrayObjId;
        }
        // Float slots: a float local's i64 var holds f64 bits; mark it Float so
        // `LoadLocal` bitcasts to F64. The ELEMENT slot is Float per `float_elem`,
        // the ACCUMULATOR slot per `float_acc` — decoupled so an Int element can
        // thread a Float accumulator (`ints.each { |x| t += x*1.5 }`). For Inject /
        // EachAcc / EachWithIndex arg2 is the accumulator and arg3 the element; for
        // a value block (None) / EachObj arg2 IS the element. (Existing callers pass
        // float_acc == float_elem ⇒ identical to the old single-flag behaviour.)
        match acc_kind {
            AccKind::Inject | AccKind::EachAcc { .. } | AccKind::EachWithIndex { .. } => {
                if float_acc {
                    local_kinds[arg2_slot as usize] = Kind::Float;
                }
                if float_elem {
                    if let Some(s) = arg3_slot {
                        local_kinds[s as usize] = Kind::Float;
                    }
                }
            }
            AccKind::None | AccKind::EachObj => {
                if float_elem {
                    local_kinds[arg2_slot as usize] = Kind::Float;
                }
            }
        }
        let mut ip = 0usize;
        while ip < n {
            if let Some(b) = blocks[ip] {
                // New block leader: pass the live operand stack across the
                // fall-through edge, then re-materialise it from this block's
                // parameters.
                if cur_open {
                    let args = block_args(&mut fb, b, &mut block_kinds[ip], &stack)?;
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
                Op::LoadConstFloat(f) => {
                    let v = fb.ins().f64const(*f);
                    stack.push((v, Kind::Float));
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
                    let raw = fb.use_var(vars[*s as usize]);
                    let k = local_kinds[*s as usize];
                    // A Float local's var holds the f64 BITS (I64); materialise the
                    // actual F64 for the operand stack.
                    if k == Kind::Float {
                        let f = fb.ins().bitcast(types::F64, MemFlagsData::new(), raw);
                        stack.push((f, Kind::Float));
                    } else {
                        stack.push((raw, k));
                    }
                }
                Op::StoreLocal(s) => {
                    let (v, k) = stack.pop()?;
                    match k {
                        Kind::Int | Kind::ArrayObjId => {
                            local_kinds[*s as usize] = k;
                            fb.def_var(vars[*s as usize], v);
                        }
                        // Store the f64 BITS into the I64 var.
                        Kind::Float => {
                            let bits = fb.ins().bitcast(types::I64, MemFlagsData::new(), v);
                            local_kinds[*s as usize] = Kind::Float;
                            fb.def_var(vars[*s as usize], bits);
                        }
                        Kind::Bool | Kind::Nil => return None,
                    }
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
                    emit_numeric_binop(&mut fb, *k, a, ka, b, kb, &mut stack, ovf_var)?;
                }
                Op::BinOpLocalLocal(k, a_slot, b_slot) => {
                    let ka = local_kinds[*a_slot as usize];
                    let kb = local_kinds[*b_slot as usize];
                    let a_raw = fb.use_var(vars[*a_slot as usize]);
                    let b_raw = fb.use_var(vars[*b_slot as usize]);
                    // A Float local's var holds f64 BITS — materialise the F64 before
                    // the numeric op (an Int local's var is the i64 value as-is).
                    let a = if ka == Kind::Float {
                        fb.ins().bitcast(types::F64, MemFlagsData::new(), a_raw)
                    } else {
                        a_raw
                    };
                    let b = if kb == Kind::Float {
                        fb.ins().bitcast(types::F64, MemFlagsData::new(), b_raw)
                    } else {
                        b_raw
                    };
                    emit_numeric_binop(&mut fb, *k, a, ka, b, kb, &mut stack, ovf_var)?;
                }
                Op::BinOpInt(k, imm) => {
                    let (a, ka) = stack.pop()?;
                    match ka {
                        Kind::Int => {
                            let b = fb.ins().iconst(types::I64, *imm);
                            emit_binop(&mut fb, *k, a, b, &mut stack, ovf_var);
                        }
                        // `float <op> int_imm` — coerce the immediate to f64 directly.
                        Kind::Float => {
                            let b = fb.ins().f64const(*imm as f64);
                            emit_binop_float(&mut fb, *k, a, b, &mut stack)?;
                        }
                        _ => return None,
                    }
                }
                Op::Jump(off) => {
                    let t = (ip as i64 + 1 + *off as i64) as usize;
                    let tb = blocks[t].unwrap();
                    let args = block_args(&mut fb, tb, &mut block_kinds[t], &stack)?;
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
                    let fall_args = block_args(&mut fb, fall, &mut block_kinds[ip + 1], &stack)?;
                    let target_args = block_args(&mut fb, target, &mut block_kinds[t], &stack)?;
                    // brif: non-zero (true) -> fall-through, zero (false) -> target.
                    fb.ins().brif(cond, fall, &fall_args, target, &target_args);
                    cur_open = false;
                }
                Op::Return => {
                    let (v, k) = stack.pop()?;
                    let v = if acc_from_slot {
                        // each-accumulator: the new accumulator is the captured
                        // acc slot's value AFTER the body — the block's own return
                        // (`v`/`k`, e.g. the `total += x` expression value) is
                        // discarded by `each`, so a `total += x; total * 2` body
                        // can't corrupt the accumulator.
                        let _ = (v, k);
                        fb.use_var(vars[arg2_slot as usize])
                    } else if predicate {
                        // A predicate block (count/select/any?/...) returns the
                        // result's TRUTHINESS as i64 0/1. Only a `Bool` (an `icmp`
                        // result) is sound to lower this way — zero-extend it. A
                        // non-Bool result (e.g. `count { |x| x }`, where every Int
                        // is truthy) must NOT be returned as its raw value — that
                        // would be summed, not counted; decline so the interpreter
                        // applies real truthiness.
                        match k {
                            Kind::Bool => fb.ins().uextend(types::I64, v),
                            _ => return None,
                        }
                    } else {
                        // Value mode: only an Int or an Array result boxes
                        // correctly at the boundary. A Bool/Nil result must NOT be
                        // returned as a raw i64 (`{ |x| x > 5 }` is true/false, not
                        // 1/0) — decline so the interpreter types it.
                        // The result KIND must match what the consuming loop expects:
                        // a Float driver (`float_elem`) needs a Float result, an Int
                        // driver an Int/Array. A mismatch (e.g. `floats.sum { |x| 5 }`
                        // — an Int result into the f64 accumulator) would reinterpret
                        // the bits as the wrong type, so it DECLINES to the generic
                        // path. Without this, a non-Float-returning block over a Float
                        // array silently corrupts (Int 5 -> 5.0e-323).
                        match k {
                            Kind::Int if !float_acc => {
                                m_nonfloat_ret = true;
                                v
                            }
                            Kind::ArrayObjId if !float_acc => {
                                returns_array = true;
                                m_nonfloat_ret = true;
                                v
                            }
                            // Return the f64 result's BITS. A Float DRIVER's loop
                            // bitcasts back; a METHOD's dispatch boxes Value::Float
                            // (returns_float). Both want the bits here.
                            Kind::Float if float_acc || is_method => {
                                m_float_ret = true;
                                fb.ins().bitcast(types::I64, MemFlagsData::new(), v)
                            }
                            _ => return None,
                        }
                    };
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
                    if ak != Kind::ArrayObjId {
                        return None;
                    }
                    match ek {
                        Kind::Int => {
                            fb.ins().call(arrpush_ref, &[vm_param, arr, elem]);
                        }
                        // Float `memo << f(x)`: push the f64 bits via the Float pusher
                        // (the operand stack holds an F64; bitcast to its i64 bits).
                        Kind::Float => {
                            let bits = fb.ins().bitcast(types::I64, MemFlagsData::new(), elem);
                            fb.ins().call(arrpushf_ref, &[vm_param, arr, bits]);
                        }
                        _ => return None,
                    }
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
                    if *name == self_name_id
                        || callee_refs.contains_key(name)
                        || float_callee_refs.contains_key(name) =>
                {
                    let (arg, ka) = stack.pop()?;
                    // Pick the specialization by the arg's kind: an Int arg calls the
                    // Int version (self-recursion or `c{cid}`); a Float arg the fparam
                    // version (`cf{cid}`), passing/returning f64 BITS through the i64
                    // ABI. A Float arg with no fparam callee (non-leaf, or a Float
                    // self-call) declines.
                    match ka {
                        Kind::Int => {
                            let fref = if *name == self_name_id {
                                self_ref
                            } else if let Some(r) = callee_refs.get(name) {
                                *r
                            } else {
                                return None; // Int arg but only a float callee exists
                            };
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
                        Kind::Float => {
                            let Some(fref) = float_callee_refs.get(name).copied() else {
                                return None; // no float specialization for this callee
                            };
                            // Pass the f64 bits; the callee returns f64 bits.
                            let bits = fb.ins().bitcast(types::I64, MemFlagsData::new(), arg);
                            let inst = fb.ins().call(fref, &[vm_param, self_param, bits]);
                            let (res, ovf) = {
                                let r = fb.inst_results(inst);
                                (r[0], r[1])
                            };
                            let cur = fb.use_var(ovf_var);
                            let nv = fb.ins().bor(cur, ovf);
                            fb.def_var(ovf_var, nv);
                            let f = fb.ins().bitcast(types::F64, MemFlagsData::new(), res);
                            stack.push((f, Kind::Float));
                        }
                        _ => return None,
                    }
                }
                // Pure unary Int primitive on the stack top (`x.abs`, `x.even?`,
                // …) — lowered inline, no call (see `emit_int_unary`).
                Op::Call(name, 0, _)
                    if is_int_unary(*name, syms) || is_float_to_int(*name, syms) =>
                {
                    let (v, k) = stack.pop()?;
                    match k {
                        Kind::Int if is_int_unary(*name, syms) => {
                            let r = emit_int_unary(&mut fb, ovf_var, *name, syms, v)?;
                            stack.push(r);
                        }
                        // floor/ceil/to_i/truncate/round on an Int is the identity.
                        Kind::Int => stack.push((v, Kind::Int)),
                        // Float `.abs` -> fabs (the common min_by distance key
                        // `(x - t).abs`); the sign predicates -> ordered fcmp against
                        // 0.0 (Bool); the conversions -> fcvt_to_sint (Int). Other
                        // unary primitives on a Float decline.
                        Kind::Float if *name == syms.abs => {
                            let r = fb.ins().fabs(v);
                            stack.push((r, Kind::Float));
                        }
                        Kind::Float if is_float_to_int(*name, syms) => {
                            let r = emit_float_to_int(&mut fb, ovf_var, *name, syms, v)?;
                            stack.push((r, Kind::Int));
                        }
                        Kind::Float if emit_float_predicate(&mut fb, *name, syms, v, &mut stack) => {}
                        _ => return None,
                    }
                }
                // The fused `x.method` op (LoadLocalCall) for the same unary
                // primitives — load the receiver local, then apply inline.
                Op::LoadLocalCall(slot, name, _)
                    if is_int_unary(*name, syms) || is_float_to_int(*name, syms) =>
                {
                    let raw = fb.use_var(vars[*slot as usize]);
                    match local_kinds[*slot as usize] {
                        Kind::Int if is_int_unary(*name, syms) => {
                            let r = emit_int_unary(&mut fb, ovf_var, *name, syms, raw)?;
                            stack.push(r);
                        }
                        Kind::Int => stack.push((raw, Kind::Int)), // conversion on Int = identity
                        // A Float local's var holds the f64 BITS — materialise the
                        // F64 before the float op (unlike Call(abs,0), whose operand
                        // is already an F64 on the stack).
                        Kind::Float if *name == syms.abs => {
                            let v = fb.ins().bitcast(types::F64, MemFlagsData::new(), raw);
                            let r = fb.ins().fabs(v);
                            stack.push((r, Kind::Float));
                        }
                        Kind::Float if is_float_to_int(*name, syms) => {
                            let v = fb.ins().bitcast(types::F64, MemFlagsData::new(), raw);
                            let r = emit_float_to_int(&mut fb, ovf_var, *name, syms, v)?;
                            stack.push((r, Kind::Int));
                        }
                        Kind::Float => {
                            let v = fb.ins().bitcast(types::F64, MemFlagsData::new(), raw);
                            if !emit_float_predicate(&mut fb, *name, syms, v, &mut stack) {
                                return None;
                            }
                        }
                        _ => return None,
                    }
                }
                Op::EnterLoop | Op::ExitLoop => {} // interpreter loop-stack bookkeeping; no native state
                _ => return None,
            }
            ip += 1;
        }
        fb.seal_all_blocks();
        fb.finalize();
    }
    // A method that returns BOTH a Float and a non-Float (Int/Array) on different
    // paths has an ambiguous box kind — decline. (Only reachable for methods; block
    // drivers force a single return kind, so m_nonfloat/m_float can't both be set.)
    let returns_float = m_float_ret;
    if returns_float && m_nonfloat_ret {
        return None;
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
        returns_float: std::cell::Cell::new(returns_float),
        _caches: caches,
    })
}

/// Which whole-loop driver a `NativeLoop` implements (ADR 0034 layer 3). All
/// share one fixed skeleton — read the length once, then a native loop that reads
/// each element, calls the already-compiled block, and acts on the result —
/// differing only in the per-element action and what the loop returns.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum LoopKind {
    /// `sum` / `count`: accumulate `acc += r` (overflow → deopt); `arg2` seeds
    /// `acc`, the result is the final `acc`.
    Sum,
    /// `map`: store `out[i] = r` into the pre-sized array `arg2`; result unused.
    Map,
    /// `select` (`keep == true`) / `reject` (`keep == false`): push the ELEMENT
    /// into the reserved array `arg2` when the predicate's polarity matches.
    Filter { keep: bool },
    /// `find` / `detect`: push the FIRST matching element into the (capacity-1)
    /// array `arg2` and early-return; the caller reads `arg2[0]` or treats an
    /// empty `arg2` as "not found". A predicate block, like `Filter`.
    Find,
}

/// A compiled native whole-loop driver (`sum` / `count` / `map` / `select` /
/// `reject`, ADR 0034 layer 3). The whole iteration runs native — walk the array,
/// call the already-compiled block per element (a tight native→native call, no
/// interpreter re-entry), act on each result — so there is NO per-element Rust
/// dispatch. ABI: `(vm, self, in_objid, arg2) -> (res, ovf)`, where `arg2` is the
/// sum seed or the output `ObjId` per `LoopKind`.
pub(crate) struct NativeLoop {
    _module: JITModule,
    ptr: extern "C" fn(*const crate::vm::Vm, *const Value, i64, i64) -> NRet,
}

impl NativeLoop {
    /// Run the driver over the (caller-pinned) Array `in_objid`. `arg2` is the sum
    /// seed (`Sum`) or the output `ObjId` (`Map` / `Filter`). `Some(res)` ran
    /// natively (the sum, or 0 for the array-producing kinds); `None` = deopt (a
    /// non-Int element, non-Int block result, or i64 overflow) → the caller redoes
    /// via the generic loop. Sound: a native block is pure, so a part-way deopt
    /// commits nothing observable (`sum` discards a register; `map`/`filter`'s
    /// partial output array is dropped).
    #[inline]
    pub(crate) fn call(
        &self,
        vm: *const crate::vm::Vm,
        recv: &Value,
        in_objid: i64,
        arg2: i64,
    ) -> Option<i64> {
        let r = (self.ptr)(vm, recv as *const Value, in_objid, arg2);
        if r.ovf == 0 {
            Some(r.res)
        } else {
            None
        }
    }
}

/// Compile a native whole-loop driver of `kind` around an already-compiled block
/// (`block_addr` from `NativeProto::addr`; a PREDICATE block for `Filter`, a value
/// block for `Sum`/`Map`). A FIXED template (not data-driven): wire `jit_array_len`
/// + a per-element `jit_array_elem_int` + a native call to the block + the kind's
/// per-element action, with one shared `deopt` exit. Only the action and the
/// carried accumulator vary by `kind`; the head always carries `(i, acc)` (the
/// array-producing kinds leave `acc` at 0).
pub(crate) fn compile_native_loop(block_addr: usize, kind: LoopKind) -> Option<NativeLoop> {
    compile_native_loop_inner(block_addr, kind, false)
}

/// Float-element variant: reads each element via `jit_array_elem_float` and (for
/// `Filter`/`Find`) pushes the matching element via `jit_array_push_float`. Used by
/// the Float `count`/`select`/`reject`/`find` drivers; the block is a FLOAT predicate
/// (param Float, returns Bool from an `fcmp`). `Sum`-kind (count) sums 0/1 as int —
/// element type doesn't matter there beyond the read.
pub(crate) fn compile_native_floatloop(block_addr: usize, kind: LoopKind) -> Option<NativeLoop> {
    compile_native_loop_inner(block_addr, kind, true)
}

fn compile_native_loop_inner(
    block_addr: usize,
    kind: LoopKind,
    float_elem: bool,
) -> Option<NativeLoop> {
    let (elem_name, elem_fn): (&str, *const u8) = if float_elem {
        ("jit_array_elem_float", jit_array_elem_float as *const u8)
    } else {
        ("jit_array_elem_int", jit_array_elem_int as *const u8)
    };
    let (push_name, push_fn): (&str, *const u8) = if float_elem {
        ("jit_array_push_float", jit_array_push_float as *const u8)
    } else {
        ("jit_array_push", jit_array_push as *const u8)
    };
    let mut builder = JITBuilder::new(cranelift_module::default_libcall_names()).ok()?;
    builder.symbol("jit_array_len", jit_array_len as *const u8);
    builder.symbol(elem_name, elem_fn);
    builder.symbol("jit_array_set_int", jit_array_set_int as *const u8);
    builder.symbol(push_name, push_fn);
    builder.symbol("blk", block_addr as *const u8);
    let mut module = JITModule::new(builder);
    let ptr_ty = module.target_config().pointer_type();
    let mut ctx = module.make_context();

    // Exported driver: (vm, self, in_objid, arg2) -> (i64 res, i8 ovf).
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(ptr_ty)); // vm
    sig.params.push(AbiParam::new(ptr_ty)); // self (block receiver)
    sig.params.push(AbiParam::new(types::I64)); // in array objid
    sig.params.push(AbiParam::new(types::I64)); // arg2: sum seed / out objid
    sig.returns.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I8));
    ctx.func.signature = sig.clone();
    let fid = module.declare_function("loopdrv", Linkage::Export, &sig).ok()?;

    // jit_array_len: (vm, objid) -> i64.
    let mut lensig = module.make_signature();
    lensig.params.push(AbiParam::new(ptr_ty));
    lensig.params.push(AbiParam::new(types::I64));
    lensig.returns.push(AbiParam::new(types::I64));
    let lenid = module
        .declare_function("jit_array_len", Linkage::Import, &lensig)
        .ok()?;
    // element reader: (vm, objid, i) -> (i64, i8). Int or Float per `float_elem`.
    let mut elsig = module.make_signature();
    elsig.params.push(AbiParam::new(ptr_ty));
    elsig.params.push(AbiParam::new(types::I64));
    elsig.params.push(AbiParam::new(types::I64));
    elsig.returns.push(AbiParam::new(types::I64));
    elsig.returns.push(AbiParam::new(types::I8));
    let elid = module
        .declare_function(elem_name, Linkage::Import, &elsig)
        .ok()?;
    // jit_array_set_int: (vm, objid, i, val) -> void (Map).
    let mut setsig = module.make_signature();
    setsig.params.push(AbiParam::new(ptr_ty));
    setsig.params.push(AbiParam::new(types::I64));
    setsig.params.push(AbiParam::new(types::I64));
    setsig.params.push(AbiParam::new(types::I64));
    let setid = module
        .declare_function("jit_array_set_int", Linkage::Import, &setsig)
        .ok()?;
    // element push: (vm, objid, elem) -> void (Filter/Find). Int or Float.
    let mut pushsig = module.make_signature();
    pushsig.params.push(AbiParam::new(ptr_ty));
    pushsig.params.push(AbiParam::new(types::I64));
    pushsig.params.push(AbiParam::new(types::I64));
    let pushid = module
        .declare_function(push_name, Linkage::Import, &pushsig)
        .ok()?;
    // block: (vm, self, i64) -> (i64, i8) — same ABI as a NativeProto.
    let mut blksig = module.make_signature();
    blksig.params.push(AbiParam::new(ptr_ty));
    blksig.params.push(AbiParam::new(ptr_ty));
    blksig.params.push(AbiParam::new(types::I64));
    blksig.returns.push(AbiParam::new(types::I64));
    blksig.returns.push(AbiParam::new(types::I8));
    let blkid = module
        .declare_function("blk", Linkage::Import, &blksig)
        .ok()?;

    let mut fbctx = FunctionBuilderContext::new();
    {
        let mut fb = FunctionBuilder::new(&mut ctx.func, &mut fbctx);
        let len_ref = module.declare_func_in_func(lenid, fb.func);
        let el_ref = module.declare_func_in_func(elid, fb.func);
        let set_ref = module.declare_func_in_func(setid, fb.func);
        let push_ref = module.declare_func_in_func(pushid, fb.func);
        let blk_ref = module.declare_func_in_func(blkid, fb.func);

        let entry = fb.create_block();
        let head = fb.create_block(); // params: (i, acc)
        let body = fb.create_block();
        let cont1 = fb.create_block();
        let cont2 = fb.create_block();
        let exit = fb.create_block(); // param: (acc)
        let deopt = fb.create_block();
        fb.append_block_params_for_function_params(entry);
        fb.append_block_param(head, types::I64); // i
        fb.append_block_param(head, types::I64); // acc
        fb.append_block_param(exit, types::I64); // acc

        // entry: len = len(in); acc0 = (Sum ? arg2 : 0); jump head(0, acc0)
        fb.switch_to_block(entry);
        let (vm_param, self_param, in_objid, arg2) = {
            let p = fb.block_params(entry);
            (p[0], p[1], p[2], p[3])
        };
        let call_len = fb.ins().call(len_ref, &[vm_param, in_objid]);
        let len = fb.inst_results(call_len)[0];
        let zero = fb.ins().iconst(types::I64, 0);
        let acc0 = match kind {
            LoopKind::Sum => arg2,
            _ => zero,
        };
        fb.ins().jump(head, &[zero.into(), acc0.into()]);

        // head(i, acc): i < len ? body : exit(acc)
        fb.switch_to_block(head);
        let (i, acc) = {
            let p = fb.block_params(head);
            (p[0], p[1])
        };
        let cond = fb.ins().icmp(IntCC::SignedLessThan, i, len);
        fb.ins().brif(cond, body, &[], exit, &[acc.into()]);

        // body: (x, ovf1) = elem_int(vm, in, i); ovf1 ? deopt : cont1
        fb.switch_to_block(body);
        let call_el = fb.ins().call(el_ref, &[vm_param, in_objid, i]);
        let (x, ovf1) = {
            let r = fb.inst_results(call_el);
            (r[0], r[1])
        };
        fb.ins().brif(ovf1, deopt, &[], cont1, &[]);

        // cont1: (r, ovf2) = blk(vm, self, x); ovf2 ? deopt : cont2
        fb.switch_to_block(cont1);
        let call_blk = fb.ins().call(blk_ref, &[vm_param, self_param, x]);
        let (r, ovf2) = {
            let res = fb.inst_results(call_blk);
            (res[0], res[1])
        };
        fb.ins().brif(ovf2, deopt, &[], cont2, &[]);

        // cont2: per-kind action, then loop to head(i+1, acc')
        fb.switch_to_block(cont2);
        let one = fb.ins().iconst(types::I64, 1);
        let i2 = fb.ins().iadd(i, one);
        match kind {
            // Sum/count: overflow-checked accumulate.
            LoopKind::Sum => {
                let (acc2, ovf3) = fb.ins().sadd_overflow(acc, r);
                let nh = fb.create_block();
                fb.ins().brif(ovf3, deopt, &[], nh, &[]);
                fb.switch_to_block(nh);
                fb.ins().jump(head, &[i2.into(), acc2.into()]);
            }
            // Map: store the result at the element's index; acc unchanged.
            LoopKind::Map => {
                fb.ins().call(set_ref, &[vm_param, arg2, i, r]);
                fb.ins().jump(head, &[i2.into(), acc.into()]);
            }
            // Filter: push the ELEMENT on the kept polarity; acc unchanged.
            LoopKind::Filter { keep } => {
                let do_push = fb.create_block();
                let skip = fb.create_block();
                let is_true = fb.ins().icmp_imm(IntCC::NotEqual, r, 0);
                if keep {
                    fb.ins().brif(is_true, do_push, &[], skip, &[]);
                } else {
                    fb.ins().brif(is_true, skip, &[], do_push, &[]);
                }
                fb.switch_to_block(do_push);
                fb.ins().call(push_ref, &[vm_param, arg2, x]);
                fb.ins().jump(skip, &[]);
                fb.switch_to_block(skip);
                fb.ins().jump(head, &[i2.into(), acc.into()]);
            }
            // Find: on the first match push the element and early-exit; otherwise
            // continue. `exit` with an empty `arg2` means "not found".
            LoopKind::Find => {
                let do_push = fb.create_block();
                let cont = fb.create_block();
                let is_true = fb.ins().icmp_imm(IntCC::NotEqual, r, 0);
                fb.ins().brif(is_true, do_push, &[], cont, &[]);
                fb.switch_to_block(do_push);
                fb.ins().call(push_ref, &[vm_param, arg2, x]);
                fb.ins().jump(exit, &[acc.into()]);
                fb.switch_to_block(cont);
                fb.ins().jump(head, &[i2.into(), acc.into()]);
            }
        }

        // exit(acc): Sum returns acc; array-producing kinds return 0.
        fb.switch_to_block(exit);
        let acc_out = fb.block_params(exit)[0];
        let ok = fb.ins().iconst(types::I8, 0);
        let res = match kind {
            LoopKind::Sum => acc_out,
            _ => fb.ins().iconst(types::I64, 0),
        };
        fb.ins().return_(&[res, ok]);

        // deopt: return (0, 1)
        fb.switch_to_block(deopt);
        let z = fb.ins().iconst(types::I64, 0);
        let bad = fb.ins().iconst(types::I8, 1);
        fb.ins().return_(&[z, bad]);

        fb.seal_all_blocks();
        fb.finalize();
    }
    module.define_function(fid, &mut ctx).ok()?;
    module.clear_context(&mut ctx);
    module.finalize_definitions().ok()?;
    let code_ptr = module.get_finalized_function(fid);
    let ptr = unsafe {
        std::mem::transmute::<_, extern "C" fn(*const crate::vm::Vm, *const Value, i64, i64) -> NRet>(
            code_ptr,
        )
    };
    Some(NativeLoop {
        _module: module,
        ptr,
    })
}

/// Compile a native whole-loop `Array#inject` / `reduce { |acc, x| .. }` driver
/// (ADR 0034 layer 3) around an already-compiled 2-param Int block (`block_addr`).
/// The block threads the accumulator: `acc = blk(acc, elem)` per element, no
/// capture. ABI `(vm, self, in_objid, init) -> (acc, ovf)`. A non-Int element or
/// any overflow inside the block deopts (the block returns the new acc directly,
/// so the loop just chains it). Returned as a `NativeLoop` (`call` gives the final
/// acc as `Some`, or `None` on deopt → the caller redoes the generic inject).
pub(crate) fn compile_native_inject_loop(block_addr: usize) -> Option<NativeLoop> {
    compile_native_inject_loop_inner(block_addr, false)
}

/// Float-element variant of the inject/each-accumulator loop: reads each element
/// via `jit_array_elem_float` (the accumulator threads as opaque i64 bits — the
/// loop never interprets it, so f64 bits flow through unchanged). Used by the Float
/// each-accumulator driver.
pub(crate) fn compile_native_floatinject_loop(block_addr: usize) -> Option<NativeLoop> {
    compile_native_inject_loop_inner(block_addr, true)
}

fn compile_native_inject_loop_inner(block_addr: usize, float_elem: bool) -> Option<NativeLoop> {
    let (elem_name, elem_fn): (&str, *const u8) = if float_elem {
        ("jit_array_elem_float", jit_array_elem_float as *const u8)
    } else {
        ("jit_array_elem_int", jit_array_elem_int as *const u8)
    };
    let mut builder = JITBuilder::new(cranelift_module::default_libcall_names()).ok()?;
    builder.symbol("jit_array_len", jit_array_len as *const u8);
    builder.symbol(elem_name, elem_fn);
    builder.symbol("blk", block_addr as *const u8);
    let mut module = JITModule::new(builder);
    let ptr_ty = module.target_config().pointer_type();
    let mut ctx = module.make_context();

    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(ptr_ty)); // vm
    sig.params.push(AbiParam::new(ptr_ty)); // self
    sig.params.push(AbiParam::new(types::I64)); // in objid
    sig.params.push(AbiParam::new(types::I64)); // init
    sig.returns.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I8));
    ctx.func.signature = sig.clone();
    let fid = module.declare_function("injectloop", Linkage::Export, &sig).ok()?;

    let mut lensig = module.make_signature();
    lensig.params.push(AbiParam::new(ptr_ty));
    lensig.params.push(AbiParam::new(types::I64));
    lensig.returns.push(AbiParam::new(types::I64));
    let lenid = module
        .declare_function("jit_array_len", Linkage::Import, &lensig)
        .ok()?;
    let mut elsig = module.make_signature();
    elsig.params.push(AbiParam::new(ptr_ty));
    elsig.params.push(AbiParam::new(types::I64));
    elsig.params.push(AbiParam::new(types::I64));
    elsig.returns.push(AbiParam::new(types::I64));
    elsig.returns.push(AbiParam::new(types::I8));
    let elid = module
        .declare_function(elem_name, Linkage::Import, &elsig)
        .ok()?;
    // 2-param block: (vm, self, acc, elem) -> (new_acc, ovf).
    let mut blksig = module.make_signature();
    blksig.params.push(AbiParam::new(ptr_ty));
    blksig.params.push(AbiParam::new(ptr_ty));
    blksig.params.push(AbiParam::new(types::I64));
    blksig.params.push(AbiParam::new(types::I64));
    blksig.returns.push(AbiParam::new(types::I64));
    blksig.returns.push(AbiParam::new(types::I8));
    let blkid = module
        .declare_function("blk", Linkage::Import, &blksig)
        .ok()?;

    let mut fbctx = FunctionBuilderContext::new();
    {
        let mut fb = FunctionBuilder::new(&mut ctx.func, &mut fbctx);
        let len_ref = module.declare_func_in_func(lenid, fb.func);
        let el_ref = module.declare_func_in_func(elid, fb.func);
        let blk_ref = module.declare_func_in_func(blkid, fb.func);

        let entry = fb.create_block();
        let head = fb.create_block(); // params: (i, acc)
        let body = fb.create_block();
        let cont1 = fb.create_block();
        let exit = fb.create_block(); // param: (acc)
        let deopt = fb.create_block();
        fb.append_block_params_for_function_params(entry);
        fb.append_block_param(head, types::I64);
        fb.append_block_param(head, types::I64);
        fb.append_block_param(exit, types::I64);

        // entry: len = len(in); jump head(0, init)
        fb.switch_to_block(entry);
        let (vm_param, self_param, in_objid, init) = {
            let p = fb.block_params(entry);
            (p[0], p[1], p[2], p[3])
        };
        let call_len = fb.ins().call(len_ref, &[vm_param, in_objid]);
        let len = fb.inst_results(call_len)[0];
        let zero = fb.ins().iconst(types::I64, 0);
        fb.ins().jump(head, &[zero.into(), init.into()]);

        // head(i, acc): i < len ? body : exit(acc)
        fb.switch_to_block(head);
        let (i, acc) = {
            let p = fb.block_params(head);
            (p[0], p[1])
        };
        let cond = fb.ins().icmp(IntCC::SignedLessThan, i, len);
        fb.ins().brif(cond, body, &[], exit, &[acc.into()]);

        // body: (x, ovf1) = elem(in, i); ovf1 ? deopt : cont1
        fb.switch_to_block(body);
        let call_el = fb.ins().call(el_ref, &[vm_param, in_objid, i]);
        let (x, ovf1) = {
            let r = fb.inst_results(call_el);
            (r[0], r[1])
        };
        fb.ins().brif(ovf1, deopt, &[], cont1, &[]);

        // cont1: (acc2, ovf2) = blk(vm, self, acc, x); ovf2 ? deopt : head(i+1, acc2)
        fb.switch_to_block(cont1);
        let call_blk = fb.ins().call(blk_ref, &[vm_param, self_param, acc, x]);
        let (acc2, ovf2) = {
            let res = fb.inst_results(call_blk);
            (res[0], res[1])
        };
        let nh = fb.create_block();
        fb.ins().brif(ovf2, deopt, &[], nh, &[]);
        fb.switch_to_block(nh);
        let one = fb.ins().iconst(types::I64, 1);
        let i2 = fb.ins().iadd(i, one);
        fb.ins().jump(head, &[i2.into(), acc2.into()]);

        // exit(acc): return (acc, 0)
        fb.switch_to_block(exit);
        let acc_out = fb.block_params(exit)[0];
        let ok = fb.ins().iconst(types::I8, 0);
        fb.ins().return_(&[acc_out, ok]);

        // deopt: return (0, 1)
        fb.switch_to_block(deopt);
        let z = fb.ins().iconst(types::I64, 0);
        let bad = fb.ins().iconst(types::I8, 1);
        fb.ins().return_(&[z, bad]);

        fb.seal_all_blocks();
        fb.finalize();
    }
    module.define_function(fid, &mut ctx).ok()?;
    module.clear_context(&mut ctx);
    module.finalize_definitions().ok()?;
    let code_ptr = module.get_finalized_function(fid);
    let ptr = unsafe {
        std::mem::transmute::<_, extern "C" fn(*const crate::vm::Vm, *const Value, i64, i64) -> NRet>(
            code_ptr,
        )
    };
    Some(NativeLoop {
        _module: module,
        ptr,
    })
}

/// Compile a native whole-loop `Array#each_with_object(coll) { |x, memo| memo <<
/// f(x) }` driver (ADR 0034 layer 3c) around an already-compiled 2-param block
/// whose `memo` param is bound to a SCRATCH array. The loop allocates the scratch
/// once, threads it as the `memo` arg (constant) to every block call — the block
/// pushes into it via the normal `<<` codegen — and returns the scratch ObjId.
/// ABI `(vm, self, in_objid, _) -> (scratch_objid, ovf)`. A non-Int element or any
/// overflow deopts (`ovf=1`); the caller then DISCARDS scratch and redoes the
/// generic `each_with_object` over the REAL memo (untouched here), so the partial
/// scratch pushes never reach the user's object — write-back-on-success.
pub(crate) fn compile_native_eachobj_loop(block_addr: usize) -> Option<NativeLoop> {
    compile_native_eachobj_loop_inner(block_addr, false)
}

/// Float-element variant (`floats.each_with_object(memo) { |x, m| m << f(x) }`): reads
/// each element via `jit_array_elem_float`. The block's `<<` pushes Int OR Float per
/// its result kind (the `<<` codegen handles both), so the scratch holds whatever
/// `f(x)` produced; the write-back appends it to the real memo unchanged.
pub(crate) fn compile_native_eachobj_loop_float(block_addr: usize) -> Option<NativeLoop> {
    compile_native_eachobj_loop_inner(block_addr, true)
}

fn compile_native_eachobj_loop_inner(block_addr: usize, float_elem: bool) -> Option<NativeLoop> {
    let (elem_name, elem_fn): (&str, *const u8) = if float_elem {
        ("jit_array_elem_float", jit_array_elem_float as *const u8)
    } else {
        ("jit_array_elem_int", jit_array_elem_int as *const u8)
    };
    let mut builder = JITBuilder::new(cranelift_module::default_libcall_names()).ok()?;
    builder.symbol("jit_array_new", jit_array_new as *const u8);
    builder.symbol("jit_array_len", jit_array_len as *const u8);
    builder.symbol(elem_name, elem_fn);
    builder.symbol("blk", block_addr as *const u8);
    let mut module = JITModule::new(builder);
    let ptr_ty = module.target_config().pointer_type();
    let mut ctx = module.make_context();

    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(ptr_ty)); // vm
    sig.params.push(AbiParam::new(ptr_ty)); // self
    sig.params.push(AbiParam::new(types::I64)); // in objid
    sig.params.push(AbiParam::new(types::I64)); // unused (ABI parity)
    sig.returns.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I8));
    ctx.func.signature = sig.clone();
    let fid = module.declare_function("eachobjloop", Linkage::Export, &sig).ok()?;

    let mut newsig = module.make_signature();
    newsig.params.push(AbiParam::new(ptr_ty));
    newsig.returns.push(AbiParam::new(types::I64));
    let newid = module
        .declare_function("jit_array_new", Linkage::Import, &newsig)
        .ok()?;
    let mut lensig = module.make_signature();
    lensig.params.push(AbiParam::new(ptr_ty));
    lensig.params.push(AbiParam::new(types::I64));
    lensig.returns.push(AbiParam::new(types::I64));
    let lenid = module
        .declare_function("jit_array_len", Linkage::Import, &lensig)
        .ok()?;
    let mut elsig = module.make_signature();
    elsig.params.push(AbiParam::new(ptr_ty));
    elsig.params.push(AbiParam::new(types::I64));
    elsig.params.push(AbiParam::new(types::I64));
    elsig.returns.push(AbiParam::new(types::I64));
    elsig.returns.push(AbiParam::new(types::I8));
    let elid = module
        .declare_function(elem_name, Linkage::Import, &elsig)
        .ok()?;
    // 2-param block: (vm, self, elem, memo) -> (ignored, ovf). The block pushes
    // f(elem) onto `memo` (the scratch array) itself.
    let mut blksig = module.make_signature();
    blksig.params.push(AbiParam::new(ptr_ty));
    blksig.params.push(AbiParam::new(ptr_ty));
    blksig.params.push(AbiParam::new(types::I64));
    blksig.params.push(AbiParam::new(types::I64));
    blksig.returns.push(AbiParam::new(types::I64));
    blksig.returns.push(AbiParam::new(types::I8));
    let blkid = module
        .declare_function("blk", Linkage::Import, &blksig)
        .ok()?;

    let mut fbctx = FunctionBuilderContext::new();
    {
        let mut fb = FunctionBuilder::new(&mut ctx.func, &mut fbctx);
        let new_ref = module.declare_func_in_func(newid, fb.func);
        let len_ref = module.declare_func_in_func(lenid, fb.func);
        let el_ref = module.declare_func_in_func(elid, fb.func);
        let blk_ref = module.declare_func_in_func(blkid, fb.func);

        let entry = fb.create_block();
        let head = fb.create_block(); // params: (i, scratch)
        let body = fb.create_block();
        let cont1 = fb.create_block();
        let exit = fb.create_block(); // param: (scratch)
        let deopt = fb.create_block();
        fb.append_block_params_for_function_params(entry);
        fb.append_block_param(head, types::I64);
        fb.append_block_param(head, types::I64);
        fb.append_block_param(exit, types::I64);

        // entry: scratch = new(); len = len(in); jump head(0, scratch)
        fb.switch_to_block(entry);
        let (vm_param, self_param, in_objid, _unused) = {
            let p = fb.block_params(entry);
            (p[0], p[1], p[2], p[3])
        };
        let call_new = fb.ins().call(new_ref, &[vm_param]);
        let scratch = fb.inst_results(call_new)[0];
        let call_len = fb.ins().call(len_ref, &[vm_param, in_objid]);
        let len = fb.inst_results(call_len)[0];
        let zero = fb.ins().iconst(types::I64, 0);
        fb.ins().jump(head, &[zero.into(), scratch.into()]);

        // head(i, scratch): i < len ? body : exit(scratch)
        fb.switch_to_block(head);
        let (i, scratch_h) = {
            let p = fb.block_params(head);
            (p[0], p[1])
        };
        let cond = fb.ins().icmp(IntCC::SignedLessThan, i, len);
        fb.ins().brif(cond, body, &[], exit, &[scratch_h.into()]);

        // body: (x, ovf1) = elem(in, i); ovf1 ? deopt : cont1
        fb.switch_to_block(body);
        let call_el = fb.ins().call(el_ref, &[vm_param, in_objid, i]);
        let (x, ovf1) = {
            let r = fb.inst_results(call_el);
            (r[0], r[1])
        };
        fb.ins().brif(ovf1, deopt, &[], cont1, &[]);

        // cont1: (_, ovf2) = blk(vm, self, x, scratch); ovf2 ? deopt : head(i+1, scratch)
        fb.switch_to_block(cont1);
        let call_blk = fb.ins().call(blk_ref, &[vm_param, self_param, x, scratch_h]);
        let ovf2 = fb.inst_results(call_blk)[1];
        let nh = fb.create_block();
        fb.ins().brif(ovf2, deopt, &[], nh, &[]);
        fb.switch_to_block(nh);
        let one = fb.ins().iconst(types::I64, 1);
        let i2 = fb.ins().iadd(i, one);
        fb.ins().jump(head, &[i2.into(), scratch_h.into()]);

        // exit(scratch): return (scratch, 0)
        fb.switch_to_block(exit);
        let scratch_out = fb.block_params(exit)[0];
        let ok = fb.ins().iconst(types::I8, 0);
        fb.ins().return_(&[scratch_out, ok]);

        // deopt: return (0, 1)
        fb.switch_to_block(deopt);
        let z = fb.ins().iconst(types::I64, 0);
        let bad = fb.ins().iconst(types::I8, 1);
        fb.ins().return_(&[z, bad]);

        fb.seal_all_blocks();
        fb.finalize();
    }
    module.define_function(fid, &mut ctx).ok()?;
    module.clear_context(&mut ctx);
    module.finalize_definitions().ok()?;
    let code_ptr = module.get_finalized_function(fid);
    let ptr = unsafe {
        std::mem::transmute::<_, extern "C" fn(*const crate::vm::Vm, *const Value, i64, i64) -> NRet>(
            code_ptr,
        )
    };
    Some(NativeLoop {
        _module: module,
        ptr,
    })
}

/// Compile a native whole-loop `Array#group_by { |x| key }` driver (ADR 0034 layer
/// 3c) around an already-compiled 1-param value block (the key function, returning
/// an Int key). Allocates a fresh result Hash, and per element pushes the element
/// into the bucket for `block(x)` via `jit_group_push`. ABI `(vm, self, in_objid, _)
/// -> (hash_objid, ovf)`. A non-Int element or non-Int key deopts (`ovf=1`); the
/// caller then DISCARDS the partial Hash and redoes the generic `group_by` — the
/// returned Hash is fresh (not user-visible until success), so this is sound
/// (write-back-on-success).
pub(crate) fn compile_native_groupby_loop(block_addr: usize) -> Option<NativeLoop> {
    compile_native_groupby_loop_inner(block_addr, false, false)
}

/// Float-element / Int-key variant (`floats.group_by { |x| x.floor }`): reads each
/// element via `jit_array_elem_float` and buckets the original Float under the block's
/// Int key via `jit_group_push_floatelem`. The key block is the shared Float-elem/
/// Int-result block (`jit_native_block_floatint`); a non-Float element or an out-of-
/// range conversion key deopts (discard-and-redo, same as the Int loop).
pub(crate) fn compile_native_floatint_groupby_loop(block_addr: usize) -> Option<NativeLoop> {
    compile_native_groupby_loop_inner(block_addr, true, false)
}

/// Float-element / Float-key variant (`floats.group_by { |x| x * 2.0 }`): the block
/// returns a Float key (the shared `jit_native_block_float`), and `jit_group_push_floatkey`
/// buckets the original Float under it with CRuby Float-key `eql?` semantics.
pub(crate) fn compile_native_floatkey_groupby_loop(block_addr: usize) -> Option<NativeLoop> {
    compile_native_groupby_loop_inner(block_addr, true, true)
}

fn compile_native_groupby_loop_inner(
    block_addr: usize,
    float_elem: bool,
    float_key: bool,
) -> Option<NativeLoop> {
    let (elem_name, elem_fn): (&str, *const u8) = if float_elem {
        ("jit_array_elem_float", jit_array_elem_float as *const u8)
    } else {
        ("jit_array_elem_int", jit_array_elem_int as *const u8)
    };
    let (push_name, push_fn): (&str, *const u8) = if float_key {
        ("jit_group_push_floatkey", jit_group_push_floatkey as *const u8)
    } else if float_elem {
        ("jit_group_push_floatelem", jit_group_push_floatelem as *const u8)
    } else {
        ("jit_group_push", jit_group_push as *const u8)
    };
    let mut builder = JITBuilder::new(cranelift_module::default_libcall_names()).ok()?;
    builder.symbol("jit_hash_new", jit_hash_new as *const u8);
    builder.symbol(push_name, push_fn);
    builder.symbol("jit_array_len", jit_array_len as *const u8);
    builder.symbol(elem_name, elem_fn);
    builder.symbol("blk", block_addr as *const u8);
    let mut module = JITModule::new(builder);
    let ptr_ty = module.target_config().pointer_type();
    let mut ctx = module.make_context();

    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(ptr_ty)); // vm
    sig.params.push(AbiParam::new(ptr_ty)); // self
    sig.params.push(AbiParam::new(types::I64)); // in objid
    sig.params.push(AbiParam::new(types::I64)); // unused (ABI parity)
    sig.returns.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I8));
    ctx.func.signature = sig.clone();
    let fid = module.declare_function("groupbyloop", Linkage::Export, &sig).ok()?;

    let mut newsig = module.make_signature();
    newsig.params.push(AbiParam::new(ptr_ty));
    newsig.returns.push(AbiParam::new(types::I64));
    let newid = module
        .declare_function("jit_hash_new", Linkage::Import, &newsig)
        .ok()?;
    let mut pushsig = module.make_signature();
    pushsig.params.push(AbiParam::new(ptr_ty));
    pushsig.params.push(AbiParam::new(types::I64));
    pushsig.params.push(AbiParam::new(types::I64));
    pushsig.params.push(AbiParam::new(types::I64));
    let pushid = module
        .declare_function(push_name, Linkage::Import, &pushsig)
        .ok()?;
    let mut lensig = module.make_signature();
    lensig.params.push(AbiParam::new(ptr_ty));
    lensig.params.push(AbiParam::new(types::I64));
    lensig.returns.push(AbiParam::new(types::I64));
    let lenid = module
        .declare_function("jit_array_len", Linkage::Import, &lensig)
        .ok()?;
    let mut elsig = module.make_signature();
    elsig.params.push(AbiParam::new(ptr_ty));
    elsig.params.push(AbiParam::new(types::I64));
    elsig.params.push(AbiParam::new(types::I64));
    elsig.returns.push(AbiParam::new(types::I64));
    elsig.returns.push(AbiParam::new(types::I8));
    let elid = module
        .declare_function(elem_name, Linkage::Import, &elsig)
        .ok()?;
    // 1-param value block: (vm, self, x) -> (key, ovf).
    let mut blksig = module.make_signature();
    blksig.params.push(AbiParam::new(ptr_ty));
    blksig.params.push(AbiParam::new(ptr_ty));
    blksig.params.push(AbiParam::new(types::I64));
    blksig.returns.push(AbiParam::new(types::I64));
    blksig.returns.push(AbiParam::new(types::I8));
    let blkid = module
        .declare_function("blk", Linkage::Import, &blksig)
        .ok()?;

    let mut fbctx = FunctionBuilderContext::new();
    {
        let mut fb = FunctionBuilder::new(&mut ctx.func, &mut fbctx);
        let new_ref = module.declare_func_in_func(newid, fb.func);
        let push_ref = module.declare_func_in_func(pushid, fb.func);
        let len_ref = module.declare_func_in_func(lenid, fb.func);
        let el_ref = module.declare_func_in_func(elid, fb.func);
        let blk_ref = module.declare_func_in_func(blkid, fb.func);

        let entry = fb.create_block();
        let head = fb.create_block(); // params: (i, hash)
        let body = fb.create_block();
        let cont1 = fb.create_block();
        let exit = fb.create_block(); // param: (hash)
        let deopt = fb.create_block();
        fb.append_block_params_for_function_params(entry);
        fb.append_block_param(head, types::I64);
        fb.append_block_param(head, types::I64);
        fb.append_block_param(exit, types::I64);

        // entry: hash = new(); len = len(in); jump head(0, hash)
        fb.switch_to_block(entry);
        let (vm_param, self_param, in_objid, _unused) = {
            let p = fb.block_params(entry);
            (p[0], p[1], p[2], p[3])
        };
        let call_new = fb.ins().call(new_ref, &[vm_param]);
        let hash = fb.inst_results(call_new)[0];
        let call_len = fb.ins().call(len_ref, &[vm_param, in_objid]);
        let len = fb.inst_results(call_len)[0];
        let zero = fb.ins().iconst(types::I64, 0);
        fb.ins().jump(head, &[zero.into(), hash.into()]);

        // head(i, hash): i < len ? body : exit(hash)
        fb.switch_to_block(head);
        let (i, hash_h) = {
            let p = fb.block_params(head);
            (p[0], p[1])
        };
        let cond = fb.ins().icmp(IntCC::SignedLessThan, i, len);
        fb.ins().brif(cond, body, &[], exit, &[hash_h.into()]);

        // body: (x, ovf1) = elem(in, i); ovf1 ? deopt : cont1
        fb.switch_to_block(body);
        let call_el = fb.ins().call(el_ref, &[vm_param, in_objid, i]);
        let (x, ovf1) = {
            let r = fb.inst_results(call_el);
            (r[0], r[1])
        };
        fb.ins().brif(ovf1, deopt, &[], cont1, &[]);

        // cont1: (key, ovf2) = blk(vm, self, x); ovf2 ? deopt :
        //        group_push(hash, key, x); head(i+1, hash)
        fb.switch_to_block(cont1);
        let call_blk = fb.ins().call(blk_ref, &[vm_param, self_param, x]);
        let (key, ovf2) = {
            let r = fb.inst_results(call_blk);
            (r[0], r[1])
        };
        let nh = fb.create_block();
        fb.ins().brif(ovf2, deopt, &[], nh, &[]);
        fb.switch_to_block(nh);
        fb.ins().call(push_ref, &[vm_param, hash_h, key, x]);
        let one = fb.ins().iconst(types::I64, 1);
        let i2 = fb.ins().iadd(i, one);
        fb.ins().jump(head, &[i2.into(), hash_h.into()]);

        // exit(hash): return (hash, 0)
        fb.switch_to_block(exit);
        let hash_out = fb.block_params(exit)[0];
        let ok = fb.ins().iconst(types::I8, 0);
        fb.ins().return_(&[hash_out, ok]);

        // deopt: return (0, 1)
        fb.switch_to_block(deopt);
        let z = fb.ins().iconst(types::I64, 0);
        let bad = fb.ins().iconst(types::I8, 1);
        fb.ins().return_(&[z, bad]);

        fb.seal_all_blocks();
        fb.finalize();
    }
    module.define_function(fid, &mut ctx).ok()?;
    module.clear_context(&mut ctx);
    module.finalize_definitions().ok()?;
    let code_ptr = module.get_finalized_function(fid);
    let ptr = unsafe {
        std::mem::transmute::<_, extern "C" fn(*const crate::vm::Vm, *const Value, i64, i64) -> NRet>(
            code_ptr,
        )
    };
    Some(NativeLoop {
        _module: module,
        ptr,
    })
}

/// Compile a native whole-loop `Array#each_with_index { |x, i| total += f(x, i) }`
/// driver (ADR 0034 layer 3c) around an already-compiled 3-input block (acc, elem,
/// index). Like the inject loop, but the block also receives the loop index `i`:
/// `acc = blk(vm, self, acc, elem, i)` per element. ABI `(vm, self, in_objid, init)
/// -> (acc, ovf)`. A non-Int element or any overflow deopts; the caller writes the
/// accumulator back to its captured slot only on full success (write-back-on-success,
/// like the each-accumulator).
pub(crate) fn compile_native_eachidx_loop(block_addr: usize) -> Option<NativeLoop> {
    let mut builder = JITBuilder::new(cranelift_module::default_libcall_names()).ok()?;
    builder.symbol("jit_array_len", jit_array_len as *const u8);
    builder.symbol("jit_array_elem_int", jit_array_elem_int as *const u8);
    builder.symbol("blk", block_addr as *const u8);
    let mut module = JITModule::new(builder);
    let ptr_ty = module.target_config().pointer_type();
    let mut ctx = module.make_context();

    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(ptr_ty)); // vm
    sig.params.push(AbiParam::new(ptr_ty)); // self
    sig.params.push(AbiParam::new(types::I64)); // in objid
    sig.params.push(AbiParam::new(types::I64)); // init (acc seed)
    sig.returns.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I8));
    ctx.func.signature = sig.clone();
    let fid = module.declare_function("eachidxloop", Linkage::Export, &sig).ok()?;

    let mut lensig = module.make_signature();
    lensig.params.push(AbiParam::new(ptr_ty));
    lensig.params.push(AbiParam::new(types::I64));
    lensig.returns.push(AbiParam::new(types::I64));
    let lenid = module
        .declare_function("jit_array_len", Linkage::Import, &lensig)
        .ok()?;
    let mut elsig = module.make_signature();
    elsig.params.push(AbiParam::new(ptr_ty));
    elsig.params.push(AbiParam::new(types::I64));
    elsig.params.push(AbiParam::new(types::I64));
    elsig.returns.push(AbiParam::new(types::I64));
    elsig.returns.push(AbiParam::new(types::I8));
    let elid = module
        .declare_function("jit_array_elem_int", Linkage::Import, &elsig)
        .ok()?;
    // 3-input block: (vm, self, acc, elem, idx) -> (new_acc, ovf).
    let mut blksig = module.make_signature();
    blksig.params.push(AbiParam::new(ptr_ty));
    blksig.params.push(AbiParam::new(ptr_ty));
    blksig.params.push(AbiParam::new(types::I64));
    blksig.params.push(AbiParam::new(types::I64));
    blksig.params.push(AbiParam::new(types::I64));
    blksig.returns.push(AbiParam::new(types::I64));
    blksig.returns.push(AbiParam::new(types::I8));
    let blkid = module
        .declare_function("blk", Linkage::Import, &blksig)
        .ok()?;

    let mut fbctx = FunctionBuilderContext::new();
    {
        let mut fb = FunctionBuilder::new(&mut ctx.func, &mut fbctx);
        let len_ref = module.declare_func_in_func(lenid, fb.func);
        let el_ref = module.declare_func_in_func(elid, fb.func);
        let blk_ref = module.declare_func_in_func(blkid, fb.func);

        let entry = fb.create_block();
        let head = fb.create_block(); // params: (i, acc)
        let body = fb.create_block();
        let cont1 = fb.create_block();
        let exit = fb.create_block(); // param: (acc)
        let deopt = fb.create_block();
        fb.append_block_params_for_function_params(entry);
        fb.append_block_param(head, types::I64);
        fb.append_block_param(head, types::I64);
        fb.append_block_param(exit, types::I64);

        // entry: len = len(in); jump head(0, init)
        fb.switch_to_block(entry);
        let (vm_param, self_param, in_objid, init) = {
            let p = fb.block_params(entry);
            (p[0], p[1], p[2], p[3])
        };
        let call_len = fb.ins().call(len_ref, &[vm_param, in_objid]);
        let len = fb.inst_results(call_len)[0];
        let zero = fb.ins().iconst(types::I64, 0);
        fb.ins().jump(head, &[zero.into(), init.into()]);

        // head(i, acc): i < len ? body : exit(acc)
        fb.switch_to_block(head);
        let (i, acc) = {
            let p = fb.block_params(head);
            (p[0], p[1])
        };
        let cond = fb.ins().icmp(IntCC::SignedLessThan, i, len);
        fb.ins().brif(cond, body, &[], exit, &[acc.into()]);

        // body: (x, ovf1) = elem(in, i); ovf1 ? deopt : cont1
        fb.switch_to_block(body);
        let call_el = fb.ins().call(el_ref, &[vm_param, in_objid, i]);
        let (x, ovf1) = {
            let r = fb.inst_results(call_el);
            (r[0], r[1])
        };
        fb.ins().brif(ovf1, deopt, &[], cont1, &[]);

        // cont1: (acc2, ovf2) = blk(vm, self, acc, x, i); ovf2 ? deopt : head(i+1, acc2)
        fb.switch_to_block(cont1);
        let call_blk = fb.ins().call(blk_ref, &[vm_param, self_param, acc, x, i]);
        let (acc2, ovf2) = {
            let res = fb.inst_results(call_blk);
            (res[0], res[1])
        };
        let nh = fb.create_block();
        fb.ins().brif(ovf2, deopt, &[], nh, &[]);
        fb.switch_to_block(nh);
        let one = fb.ins().iconst(types::I64, 1);
        let i2 = fb.ins().iadd(i, one);
        fb.ins().jump(head, &[i2.into(), acc2.into()]);

        // exit(acc): return (acc, 0)
        fb.switch_to_block(exit);
        let acc_out = fb.block_params(exit)[0];
        let ok = fb.ins().iconst(types::I8, 0);
        fb.ins().return_(&[acc_out, ok]);

        // deopt: return (0, 1)
        fb.switch_to_block(deopt);
        let z = fb.ins().iconst(types::I64, 0);
        let bad = fb.ins().iconst(types::I8, 1);
        fb.ins().return_(&[z, bad]);

        fb.seal_all_blocks();
        fb.finalize();
    }
    module.define_function(fid, &mut ctx).ok()?;
    module.clear_context(&mut ctx);
    module.finalize_definitions().ok()?;
    let code_ptr = module.get_finalized_function(fid);
    let ptr = unsafe {
        std::mem::transmute::<_, extern "C" fn(*const crate::vm::Vm, *const Value, i64, i64) -> NRet>(
            code_ptr,
        )
    };
    Some(NativeLoop {
        _module: module,
        ptr,
    })
}

/// Compile a native whole-loop `Array#sum { |x| f(x) }` driver over an all-FLOAT
/// array (ADR 0034 layer 3d — first Float driver). The block is a 1-param value
/// block whose element + result are f64 BITS in the i64 ABI; the loop reads each
/// element via `jit_array_elem_float` (deopt on non-Float), runs the block, and
/// `fadd`-accumulates (no overflow — IEEE). ABI `(vm, self, in_objid, init_bits)
/// -> (sum_bits, ovf)`; the caller seeds `init_bits` (the `sum(init)` argument's
/// f64 bits) and boxes the result as `Value::Float(f64::from_bits(res))`. A non-Float
/// element deopts → the caller redoes the generic sum.
pub(crate) fn compile_native_floatsum_loop(block_addr: usize) -> Option<NativeLoop> {
    compile_native_floatsum_loop_inner(block_addr, false)
}

/// `int_elem`: read elements as Int (the block takes an Int param, produces a Float
/// — `ints.sum { |x| x * 1.5 }`); else read as Float (`floats.sum { ... }`). Either
/// way the accumulator is f64 and the block returns f64 BITS, so only the element
/// reader symbol differs.
pub(crate) fn compile_native_floatsum_loop_inner(
    block_addr: usize,
    int_elem: bool,
) -> Option<NativeLoop> {
    let mut builder = JITBuilder::new(cranelift_module::default_libcall_names()).ok()?;
    let (elem_name, elem_fn): (&str, *const u8) = if int_elem {
        ("jit_array_elem_int", jit_array_elem_int as *const u8)
    } else {
        ("jit_array_elem_float", jit_array_elem_float as *const u8)
    };
    builder.symbol("jit_array_len", jit_array_len as *const u8);
    builder.symbol(elem_name, elem_fn);
    builder.symbol("blk", block_addr as *const u8);
    let mut module = JITModule::new(builder);
    let ptr_ty = module.target_config().pointer_type();
    let mut ctx = module.make_context();

    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(ptr_ty)); // vm
    sig.params.push(AbiParam::new(ptr_ty)); // self
    sig.params.push(AbiParam::new(types::I64)); // in objid
    sig.params.push(AbiParam::new(types::I64)); // init (f64 bits)
    sig.returns.push(AbiParam::new(types::I64)); // sum (f64 bits)
    sig.returns.push(AbiParam::new(types::I8));
    ctx.func.signature = sig.clone();
    let fid = module.declare_function("floatsumloop", Linkage::Export, &sig).ok()?;

    let mut lensig = module.make_signature();
    lensig.params.push(AbiParam::new(ptr_ty));
    lensig.params.push(AbiParam::new(types::I64));
    lensig.returns.push(AbiParam::new(types::I64));
    let lenid = module
        .declare_function("jit_array_len", Linkage::Import, &lensig)
        .ok()?;
    let mut elsig = module.make_signature();
    elsig.params.push(AbiParam::new(ptr_ty));
    elsig.params.push(AbiParam::new(types::I64));
    elsig.params.push(AbiParam::new(types::I64));
    elsig.returns.push(AbiParam::new(types::I64)); // f64 bits (or int value if int_elem)
    elsig.returns.push(AbiParam::new(types::I8));
    let elid = module
        .declare_function(elem_name, Linkage::Import, &elsig)
        .ok()?;
    // 1-param value block: (vm, self, elem_bits) -> (result_bits, ovf).
    let mut blksig = module.make_signature();
    blksig.params.push(AbiParam::new(ptr_ty));
    blksig.params.push(AbiParam::new(ptr_ty));
    blksig.params.push(AbiParam::new(types::I64));
    blksig.returns.push(AbiParam::new(types::I64));
    blksig.returns.push(AbiParam::new(types::I8));
    let blkid = module
        .declare_function("blk", Linkage::Import, &blksig)
        .ok()?;

    let mut fbctx = FunctionBuilderContext::new();
    {
        let mut fb = FunctionBuilder::new(&mut ctx.func, &mut fbctx);
        let len_ref = module.declare_func_in_func(lenid, fb.func);
        let el_ref = module.declare_func_in_func(elid, fb.func);
        let blk_ref = module.declare_func_in_func(blkid, fb.func);

        let entry = fb.create_block();
        let head = fb.create_block(); // params: (i: I64, acc: F64)
        let body = fb.create_block();
        let cont1 = fb.create_block();
        let exit = fb.create_block(); // param: (acc: F64)
        let deopt = fb.create_block();
        fb.append_block_params_for_function_params(entry);
        fb.append_block_param(head, types::I64);
        fb.append_block_param(head, types::F64);
        fb.append_block_param(exit, types::F64);

        // entry: len = len(in); acc0 = bitcast(init_bits); jump head(0, acc0)
        fb.switch_to_block(entry);
        let (vm_param, self_param, in_objid, init_bits) = {
            let p = fb.block_params(entry);
            (p[0], p[1], p[2], p[3])
        };
        let call_len = fb.ins().call(len_ref, &[vm_param, in_objid]);
        let len = fb.inst_results(call_len)[0];
        let acc0 = fb.ins().bitcast(types::F64, MemFlagsData::new(), init_bits);
        let zero = fb.ins().iconst(types::I64, 0);
        fb.ins().jump(head, &[zero.into(), acc0.into()]);

        // head(i, acc): i < len ? body : exit(acc)
        fb.switch_to_block(head);
        let (i, acc) = {
            let p = fb.block_params(head);
            (p[0], p[1])
        };
        let cond = fb.ins().icmp(IntCC::SignedLessThan, i, len);
        fb.ins().brif(cond, body, &[], exit, &[acc.into()]);

        // body: (xbits, ovf1) = elem_float(in, i); ovf1 ? deopt : cont1
        fb.switch_to_block(body);
        let call_el = fb.ins().call(el_ref, &[vm_param, in_objid, i]);
        let (xbits, ovf1) = {
            let r = fb.inst_results(call_el);
            (r[0], r[1])
        };
        fb.ins().brif(ovf1, deopt, &[], cont1, &[]);

        // cont1: (rbits, ovf2) = blk(vm, self, xbits); ovf2 ? deopt :
        //        acc2 = acc + bitcast(rbits); head(i+1, acc2)
        fb.switch_to_block(cont1);
        let call_blk = fb.ins().call(blk_ref, &[vm_param, self_param, xbits]);
        let (rbits, ovf2) = {
            let r = fb.inst_results(call_blk);
            (r[0], r[1])
        };
        let nh = fb.create_block();
        fb.ins().brif(ovf2, deopt, &[], nh, &[]);
        fb.switch_to_block(nh);
        let rf = fb.ins().bitcast(types::F64, MemFlagsData::new(), rbits);
        let acc2 = fb.ins().fadd(acc, rf);
        let one = fb.ins().iconst(types::I64, 1);
        let i2 = fb.ins().iadd(i, one);
        fb.ins().jump(head, &[i2.into(), acc2.into()]);

        // exit(acc): return (bitcast(acc), 0)
        fb.switch_to_block(exit);
        let acc_out = fb.block_params(exit)[0];
        let bits = fb.ins().bitcast(types::I64, MemFlagsData::new(), acc_out);
        let ok = fb.ins().iconst(types::I8, 0);
        fb.ins().return_(&[bits, ok]);

        // deopt: return (0, 1)
        fb.switch_to_block(deopt);
        let z = fb.ins().iconst(types::I64, 0);
        let bad = fb.ins().iconst(types::I8, 1);
        fb.ins().return_(&[z, bad]);

        fb.seal_all_blocks();
        fb.finalize();
    }
    module.define_function(fid, &mut ctx).ok()?;
    module.clear_context(&mut ctx);
    module.finalize_definitions().ok()?;
    let code_ptr = module.get_finalized_function(fid);
    let ptr = unsafe {
        std::mem::transmute::<_, extern "C" fn(*const crate::vm::Vm, *const Value, i64, i64) -> NRet>(
            code_ptr,
        )
    };
    Some(NativeLoop {
        _module: module,
        ptr,
    })
}

/// Compile a native whole-loop `Array#map { |x| f(x) }` driver over an all-FLOAT
/// array producing a FLOAT array (ADR 0034 layer 3d). The block's element + result
/// are f64 BITS in the i64 ABI; the loop reads each element via
/// `jit_array_elem_float` (deopt on non-Float), runs the block, and stores the
/// result bits via `jit_array_set_float` into the caller's pre-sized output array.
/// ABI `(vm, self, in_objid, out_objid) -> (0, ovf)`; the caller returns `out` on
/// success. A non-Float element deopts -> the caller discards `out` and redoes the
/// map generically (the native block is pure).
pub(crate) fn compile_native_floatmap_loop(block_addr: usize) -> Option<NativeLoop> {
    compile_native_floatmap_loop_inner(block_addr, false)
}

/// `int_elem`: read elements as Int (the block takes an Int param, produces a Float
/// — `ints.map { |x| x * 1.5 }`); else read as Float. The output is a Float array
/// either way (`jit_array_set_float`), so only the element reader symbol differs.
pub(crate) fn compile_native_floatmap_loop_inner(
    block_addr: usize,
    int_elem: bool,
) -> Option<NativeLoop> {
    let mut builder = JITBuilder::new(cranelift_module::default_libcall_names()).ok()?;
    let (elem_name, elem_fn): (&str, *const u8) = if int_elem {
        ("jit_array_elem_int", jit_array_elem_int as *const u8)
    } else {
        ("jit_array_elem_float", jit_array_elem_float as *const u8)
    };
    builder.symbol("jit_array_len", jit_array_len as *const u8);
    builder.symbol(elem_name, elem_fn);
    builder.symbol("jit_array_set_float", jit_array_set_float as *const u8);
    builder.symbol("blk", block_addr as *const u8);
    let mut module = JITModule::new(builder);
    let ptr_ty = module.target_config().pointer_type();
    let mut ctx = module.make_context();

    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(ptr_ty)); // vm
    sig.params.push(AbiParam::new(ptr_ty)); // self
    sig.params.push(AbiParam::new(types::I64)); // in objid
    sig.params.push(AbiParam::new(types::I64)); // out objid
    sig.returns.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I8));
    ctx.func.signature = sig.clone();
    let fid = module.declare_function("floatmaploop", Linkage::Export, &sig).ok()?;

    let mut lensig = module.make_signature();
    lensig.params.push(AbiParam::new(ptr_ty));
    lensig.params.push(AbiParam::new(types::I64));
    lensig.returns.push(AbiParam::new(types::I64));
    let lenid = module
        .declare_function("jit_array_len", Linkage::Import, &lensig)
        .ok()?;
    let mut elsig = module.make_signature();
    elsig.params.push(AbiParam::new(ptr_ty));
    elsig.params.push(AbiParam::new(types::I64));
    elsig.params.push(AbiParam::new(types::I64));
    elsig.returns.push(AbiParam::new(types::I64));
    elsig.returns.push(AbiParam::new(types::I8));
    let elid = module
        .declare_function(elem_name, Linkage::Import, &elsig)
        .ok()?;
    let mut setsig = module.make_signature();
    setsig.params.push(AbiParam::new(ptr_ty));
    setsig.params.push(AbiParam::new(types::I64));
    setsig.params.push(AbiParam::new(types::I64));
    setsig.params.push(AbiParam::new(types::I64));
    let setid = module
        .declare_function("jit_array_set_float", Linkage::Import, &setsig)
        .ok()?;
    let mut blksig = module.make_signature();
    blksig.params.push(AbiParam::new(ptr_ty));
    blksig.params.push(AbiParam::new(ptr_ty));
    blksig.params.push(AbiParam::new(types::I64));
    blksig.returns.push(AbiParam::new(types::I64));
    blksig.returns.push(AbiParam::new(types::I8));
    let blkid = module
        .declare_function("blk", Linkage::Import, &blksig)
        .ok()?;

    let mut fbctx = FunctionBuilderContext::new();
    {
        let mut fb = FunctionBuilder::new(&mut ctx.func, &mut fbctx);
        let len_ref = module.declare_func_in_func(lenid, fb.func);
        let el_ref = module.declare_func_in_func(elid, fb.func);
        let set_ref = module.declare_func_in_func(setid, fb.func);
        let blk_ref = module.declare_func_in_func(blkid, fb.func);

        let entry = fb.create_block();
        let head = fb.create_block(); // param: (i: I64)
        let body = fb.create_block();
        let cont1 = fb.create_block();
        let exit = fb.create_block();
        let deopt = fb.create_block();
        fb.append_block_params_for_function_params(entry);
        fb.append_block_param(head, types::I64);

        fb.switch_to_block(entry);
        let (vm_param, self_param, in_objid, out_objid) = {
            let p = fb.block_params(entry);
            (p[0], p[1], p[2], p[3])
        };
        let call_len = fb.ins().call(len_ref, &[vm_param, in_objid]);
        let len = fb.inst_results(call_len)[0];
        let zero = fb.ins().iconst(types::I64, 0);
        fb.ins().jump(head, &[zero.into()]);

        fb.switch_to_block(head);
        let i = fb.block_params(head)[0];
        let cond = fb.ins().icmp(IntCC::SignedLessThan, i, len);
        fb.ins().brif(cond, body, &[], exit, &[]);

        fb.switch_to_block(body);
        let call_el = fb.ins().call(el_ref, &[vm_param, in_objid, i]);
        let (xbits, ovf1) = {
            let r = fb.inst_results(call_el);
            (r[0], r[1])
        };
        fb.ins().brif(ovf1, deopt, &[], cont1, &[]);

        fb.switch_to_block(cont1);
        let call_blk = fb.ins().call(blk_ref, &[vm_param, self_param, xbits]);
        let (rbits, ovf2) = {
            let r = fb.inst_results(call_blk);
            (r[0], r[1])
        };
        let nh = fb.create_block();
        fb.ins().brif(ovf2, deopt, &[], nh, &[]);
        fb.switch_to_block(nh);
        fb.ins().call(set_ref, &[vm_param, out_objid, i, rbits]);
        let one = fb.ins().iconst(types::I64, 1);
        let i2 = fb.ins().iadd(i, one);
        fb.ins().jump(head, &[i2.into()]);

        fb.switch_to_block(exit);
        let z0 = fb.ins().iconst(types::I64, 0);
        let ok = fb.ins().iconst(types::I8, 0);
        fb.ins().return_(&[z0, ok]);

        fb.switch_to_block(deopt);
        let z = fb.ins().iconst(types::I64, 0);
        let bad = fb.ins().iconst(types::I8, 1);
        fb.ins().return_(&[z, bad]);

        fb.seal_all_blocks();
        fb.finalize();
    }
    module.define_function(fid, &mut ctx).ok()?;
    module.clear_context(&mut ctx);
    module.finalize_definitions().ok()?;
    let code_ptr = module.get_finalized_function(fid);
    let ptr = unsafe {
        std::mem::transmute::<_, extern "C" fn(*const crate::vm::Vm, *const Value, i64, i64) -> NRet>(
            code_ptr,
        )
    };
    Some(NativeLoop {
        _module: module,
        ptr,
    })
}

/// Compile a native whole-loop `Array#min_by` / `max_by { |x| key }` driver (ADR
/// 0034 layer 3) around an already-compiled 1-param Int block (the key function).
/// A fold tracking the best KEY and its ELEMENT in registers — no mutation, so a
/// mid-loop deopt commits nothing and redo-from-scratch stays sound. The caller
/// guarantees `len >= 1` (it returns nil for the empty array itself). ABI
/// `(vm, self, in_objid, _) -> (best_elem, ovf)`; `is_min` picks `<` vs `>`.
pub(crate) fn compile_native_minmax_loop(block_addr: usize, is_min: bool) -> Option<NativeLoop> {
    compile_native_minmax_loop_inner(block_addr, is_min, false, false)
}

/// Float variant: the ELEMENT and the block's KEY are Floats (f64 bits). Keys are
/// compared with ordered `fcmp` (after bitcast); a NaN key deopts (`fcmp Unordered`)
/// so CRuby's "comparison failed" raise happens on the generic path. The best
/// element threads as f64 bits; the driver boxes `Value::Float`.
pub(crate) fn compile_native_floatminmax_loop(block_addr: usize, is_min: bool) -> Option<NativeLoop> {
    compile_native_minmax_loop_inner(block_addr, is_min, true, true)
}

/// Int-element / Float-KEY min_by/max_by (`ints.min_by { |x| x*1.5 }`): the element
/// reads as an Int (and is returned as one), but the comparison KEY the block
/// produces is a Float (ordered fcmp + NaN-deopt). Decouples element-kind from
/// key-kind — the element threads as opaque i64 either way.
pub(crate) fn compile_native_intelem_floatminmax_loop(block_addr: usize, is_min: bool) -> Option<NativeLoop> {
    compile_native_minmax_loop_inner(block_addr, is_min, false, true)
}

fn compile_native_minmax_loop_inner(
    block_addr: usize,
    is_min: bool,
    float_elem: bool,
    float_key: bool,
) -> Option<NativeLoop> {
    let (elem_name, elem_fn): (&str, *const u8) = if float_elem {
        ("jit_array_elem_float", jit_array_elem_float as *const u8)
    } else {
        ("jit_array_elem_int", jit_array_elem_int as *const u8)
    };
    let mut builder = JITBuilder::new(cranelift_module::default_libcall_names()).ok()?;
    builder.symbol("jit_array_len", jit_array_len as *const u8);
    builder.symbol(elem_name, elem_fn);
    builder.symbol("blk", block_addr as *const u8);
    let mut module = JITModule::new(builder);
    let ptr_ty = module.target_config().pointer_type();
    let mut ctx = module.make_context();

    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(ptr_ty)); // vm
    sig.params.push(AbiParam::new(ptr_ty)); // self
    sig.params.push(AbiParam::new(types::I64)); // in objid
    sig.params.push(AbiParam::new(types::I64)); // unused (ABI parity)
    sig.returns.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I8));
    ctx.func.signature = sig.clone();
    let fid = module.declare_function("minmaxloop", Linkage::Export, &sig).ok()?;

    let mut lensig = module.make_signature();
    lensig.params.push(AbiParam::new(ptr_ty));
    lensig.params.push(AbiParam::new(types::I64));
    lensig.returns.push(AbiParam::new(types::I64));
    let lenid = module
        .declare_function("jit_array_len", Linkage::Import, &lensig)
        .ok()?;
    let mut elsig = module.make_signature();
    elsig.params.push(AbiParam::new(ptr_ty));
    elsig.params.push(AbiParam::new(types::I64));
    elsig.params.push(AbiParam::new(types::I64));
    elsig.returns.push(AbiParam::new(types::I64));
    elsig.returns.push(AbiParam::new(types::I8));
    let elid = module
        .declare_function(elem_name, Linkage::Import, &elsig)
        .ok()?;
    let mut blksig = module.make_signature();
    blksig.params.push(AbiParam::new(ptr_ty));
    blksig.params.push(AbiParam::new(ptr_ty));
    blksig.params.push(AbiParam::new(types::I64));
    blksig.returns.push(AbiParam::new(types::I64));
    blksig.returns.push(AbiParam::new(types::I8));
    let blkid = module
        .declare_function("blk", Linkage::Import, &blksig)
        .ok()?;

    let mut fbctx = FunctionBuilderContext::new();
    {
        let mut fb = FunctionBuilder::new(&mut ctx.func, &mut fbctx);
        let len_ref = module.declare_func_in_func(lenid, fb.func);
        let el_ref = module.declare_func_in_func(elid, fb.func);
        let blk_ref = module.declare_func_in_func(blkid, fb.func);

        let entry = fb.create_block();
        let k0blk = fb.create_block(); // after elem(0): compute its key
        let head = fb.create_block(); // params: (i, best_key, best_elem)
        let body = fb.create_block();
        let cont1 = fb.create_block();
        let exit = fb.create_block(); // param: (best_elem)
        let deopt = fb.create_block();
        fb.append_block_params_for_function_params(entry);
        fb.append_block_param(head, types::I64); // i
        fb.append_block_param(head, types::I64); // best_key
        fb.append_block_param(head, types::I64); // best_elem
        fb.append_block_param(exit, types::I64); // best_elem

        // entry: x0 = elem(in, 0); ovf? deopt : k0blk(x0)
        fb.switch_to_block(entry);
        let (vm_param, self_param, in_objid) = {
            let p = fb.block_params(entry);
            (p[0], p[1], p[2])
        };
        let zero = fb.ins().iconst(types::I64, 0);
        let call_len = fb.ins().call(len_ref, &[vm_param, in_objid]);
        let len = fb.inst_results(call_len)[0];
        let call_e0 = fb.ins().call(el_ref, &[vm_param, in_objid, zero]);
        let (x0, ovf0) = {
            let r = fb.inst_results(call_e0);
            (r[0], r[1])
        };
        // x0 flows into k0blk via a block param.
        fb.append_block_param(k0blk, types::I64);
        fb.ins().brif(ovf0, deopt, &[], k0blk, &[x0.into()]);

        // k0blk(x0): k0 = blk(x0); ovf? deopt : head(1, k0, x0)
        fb.switch_to_block(k0blk);
        let x0b = fb.block_params(k0blk)[0];
        let call_k0 = fb.ins().call(blk_ref, &[vm_param, self_param, x0b]);
        let (k0, ovfk0) = {
            let r = fb.inst_results(call_k0);
            (r[0], r[1])
        };
        let one0 = fb.ins().iconst(types::I64, 1);
        let nh0 = fb.create_block();
        fb.ins().brif(ovfk0, deopt, &[], nh0, &[]);
        fb.switch_to_block(nh0);
        // Seed key NaN -> deopt too (else the NaN element would seed `best` and
        // never be displaced, masking CRuby's "comparison failed" raise).
        if float_key {
            let k0f = fb.ins().bitcast(types::F64, MemFlagsData::new(), k0);
            let is_nan0 = fb.ins().fcmp(FloatCC::Unordered, k0f, k0f);
            let seedok = fb.create_block();
            fb.ins().brif(is_nan0, deopt, &[], seedok, &[]);
            fb.switch_to_block(seedok);
        }
        fb.ins().jump(head, &[one0.into(), k0.into(), x0b.into()]);

        // head(i, bk, be): i < len ? body : exit(be)
        fb.switch_to_block(head);
        let (i, bk, be) = {
            let p = fb.block_params(head);
            (p[0], p[1], p[2])
        };
        let cond = fb.ins().icmp(IntCC::SignedLessThan, i, len);
        fb.ins().brif(cond, body, &[], exit, &[be.into()]);

        // body: x = elem(in, i); ovf? deopt : cont1
        fb.switch_to_block(body);
        let call_el = fb.ins().call(el_ref, &[vm_param, in_objid, i]);
        let (x, ovf1) = {
            let r = fb.inst_results(call_el);
            (r[0], r[1])
        };
        fb.ins().brif(ovf1, deopt, &[], cont1, &[]);

        // cont1: k = blk(x); ovf? deopt : update best by polarity; head(i+1,..)
        fb.switch_to_block(cont1);
        let call_blk = fb.ins().call(blk_ref, &[vm_param, self_param, x]);
        let (k, ovf2) = {
            let r = fb.inst_results(call_blk);
            (r[0], r[1])
        };
        let nh = fb.create_block();
        fb.ins().brif(ovf2, deopt, &[], nh, &[]);
        fb.switch_to_block(nh);
        // better = is_min ? k < bk : k > bk; select new best key + element.
        let better = if float_key {
            // NaN key -> deopt (CRuby raises "comparison failed"; the generic
            // path reproduces that). Then ordered fcmp on the bitcast keys.
            let kf = fb.ins().bitcast(types::F64, MemFlagsData::new(), k);
            let is_nan = fb.ins().fcmp(FloatCC::Unordered, kf, kf);
            let okb = fb.create_block();
            fb.ins().brif(is_nan, deopt, &[], okb, &[]);
            fb.switch_to_block(okb);
            let bkf = fb.ins().bitcast(types::F64, MemFlagsData::new(), bk);
            let cc = if is_min { FloatCC::LessThan } else { FloatCC::GreaterThan };
            fb.ins().fcmp(cc, kf, bkf)
        } else {
            let cc = if is_min {
                IntCC::SignedLessThan
            } else {
                IntCC::SignedGreaterThan
            };
            fb.ins().icmp(cc, k, bk)
        };
        let bk2 = fb.ins().select(better, k, bk);
        let be2 = fb.ins().select(better, x, be);
        let one = fb.ins().iconst(types::I64, 1);
        let i2 = fb.ins().iadd(i, one);
        fb.ins().jump(head, &[i2.into(), bk2.into(), be2.into()]);

        // exit(be): return (be, 0)
        fb.switch_to_block(exit);
        let be_out = fb.block_params(exit)[0];
        let ok = fb.ins().iconst(types::I8, 0);
        fb.ins().return_(&[be_out, ok]);

        // deopt: return (0, 1)
        fb.switch_to_block(deopt);
        let z = fb.ins().iconst(types::I64, 0);
        let bad = fb.ins().iconst(types::I8, 1);
        fb.ins().return_(&[z, bad]);

        fb.seal_all_blocks();
        fb.finalize();
    }
    module.define_function(fid, &mut ctx).ok()?;
    module.clear_context(&mut ctx);
    module.finalize_definitions().ok()?;
    let code_ptr = module.get_finalized_function(fid);
    let ptr = unsafe {
        std::mem::transmute::<_, extern "C" fn(*const crate::vm::Vm, *const Value, i64, i64) -> NRet>(
            code_ptr,
        )
    };
    Some(NativeLoop {
        _module: module,
        ptr,
    })
}

/// Compute the block-call arguments for branching to `block` with the current
/// operand `stack` live. The FIRST branch fixes the block's parameter count +
/// kinds (`kinds_slot`); a LATER branch that arrives with a DIFFERENT kind shape
/// returns `None` to decline the whole proto. This matters because not all merges
/// are uniform: `x*2 if x.even?` merges an `Int` (the then-value) with a `Nil`
/// (the missing-else value) at one block — lowering both to a single i64 param
/// would silently treat the `nil` branch as `0`, so we must decline instead.
fn block_args(
    fb: &mut FunctionBuilder,
    block: Block,
    kinds_slot: &mut Option<Vec<Kind>>,
    stack: &[(ClValue, Kind)],
) -> Option<Vec<BlockArg>> {
    match kinds_slot {
        None => {
            for _ in stack {
                fb.append_block_param(block, types::I64);
            }
            *kinds_slot = Some(stack.iter().map(|(_, k)| *k).collect());
        }
        Some(prev) => {
            if prev.len() != stack.len() || prev.iter().zip(stack).any(|(p, (_, k))| p != k) {
                return None;
            }
        }
    }
    Some(stack.iter().map(|(v, _)| (*v).into()).collect())
}

/// Is `name` a pure unary Int primitive the JIT lowers inline (no call)?
fn is_int_unary(name: SymId, syms: &JitSyms) -> bool {
    name == syms.abs
        || name == syms.even_p
        || name == syms.odd_p
        || name == syms.zero_p
        || name == syms.positive_p
        || name == syms.negative_p
}

/// Lower a pure unary Int primitive on `v` to inline Cranelift IR. `abs` is an
/// `Int` (its `i64::MIN` negation overflows → deopt via `ovf_var`); the predicates
/// are `Bool` (an `icmp` result), usable as a comparison condition or a
/// predicate-block result. Returns `None` for an unrecognised name.
fn emit_int_unary(
    fb: &mut FunctionBuilder,
    ovf_var: Variable,
    name: SymId,
    syms: &JitSyms,
    v: ClValue,
) -> Option<(ClValue, Kind)> {
    if name == syms.abs {
        let zero = fb.ins().iconst(types::I64, 0);
        let (neg, of) = fb.ins().ssub_overflow(zero, v);
        let cur = fb.use_var(ovf_var);
        let nv = fb.ins().bor(cur, of);
        fb.def_var(ovf_var, nv);
        let isneg = fb.ins().icmp_imm(IntCC::SignedLessThan, v, 0);
        Some((fb.ins().select(isneg, neg, v), Kind::Int))
    } else if name == syms.even_p || name == syms.odd_p {
        let bit = fb.ins().band_imm(v, 1);
        let cc = if name == syms.even_p {
            IntCC::Equal
        } else {
            IntCC::NotEqual
        };
        Some((fb.ins().icmp_imm(cc, bit, 0), Kind::Bool))
    } else if name == syms.zero_p || name == syms.positive_p || name == syms.negative_p {
        let cc = if name == syms.zero_p {
            IntCC::Equal
        } else if name == syms.positive_p {
            IntCC::SignedGreaterThan
        } else {
            IntCC::SignedLessThan
        };
        Some((fb.ins().icmp_imm(cc, v, 0), Kind::Bool))
    } else {
        None
    }
}

/// Lower a Float<->Float binary op (`a`, `b` are F64). Only the four arithmetic
/// ops are modelled (no overflow — IEEE saturates to ±inf); pushes a `Float`.
/// Float comparisons + `%` (fmod) decline (`None`) — not needed by `sum`/`map`,
/// and float comparison would need careful NaN ordering. The Int `emit_binop`
/// stays the exclusive Int path; this never runs for Int operands.
/// Lower a numeric binary op with Int<->Float coercion (Ruby semantics: any Float
/// operand promotes the other). `a`/`b` are materialised values whose kinds are
/// `ka`/`kb` (a Float operand is already an F64, an Int operand an i64). Int+Int
/// stays the overflow-checked Int path; if EITHER side is Float, the Int side is
/// `fcvt_from_sint`-coerced to F64 and the float op runs. A non-numeric operand
/// (Bool/Nil/Array) declines. This is what lets `floats.sum { |x| x * 2 }` (Float
/// element, Int literal) go native.
fn emit_numeric_binop(
    fb: &mut FunctionBuilder,
    k: BinOpKind,
    a: ClValue,
    ka: Kind,
    b: ClValue,
    kb: Kind,
    stack: &mut Vec<(ClValue, Kind)>,
    ovf_var: Variable,
) -> Option<()> {
    match (ka, kb) {
        (Kind::Int, Kind::Int) => {
            emit_binop(fb, k, a, b, stack, ovf_var);
            Some(())
        }
        (ka, kb)
            if matches!(ka, Kind::Int | Kind::Float) && matches!(kb, Kind::Int | Kind::Float) =>
        {
            let af = if ka == Kind::Int {
                fb.ins().fcvt_from_sint(types::F64, a)
            } else {
                a
            };
            let bf = if kb == Kind::Int {
                fb.ins().fcvt_from_sint(types::F64, b)
            } else {
                b
            };
            emit_binop_float(fb, k, af, bf, stack)
        }
        _ => None,
    }
}

/// Float sign predicates (`x.positive?`/`negative?`/`zero?`) -> ordered fcmp against
/// 0.0, pushing a Bool. Ordered cc means NaN -> false (NaN is neither positive nor
/// negative nor zero), matching Ruby. Returns `true` if `name` was a handled
/// predicate (and pushed a Bool); `false` otherwise (caller declines).
fn emit_float_predicate(
    fb: &mut FunctionBuilder,
    name: SymId,
    syms: &JitSyms,
    v: ClValue,
    stack: &mut Vec<(ClValue, Kind)>,
) -> bool {
    let cc = if name == syms.positive_p {
        FloatCC::GreaterThan
    } else if name == syms.negative_p {
        FloatCC::LessThan
    } else if name == syms.zero_p {
        FloatCC::Equal
    } else {
        return false;
    };
    let zero = fb.ins().f64const(0.0);
    let r = fb.ins().fcmp(cc, v, zero);
    stack.push((r, Kind::Bool));
    true
}

fn is_float_to_int(name: SymId, syms: &JitSyms) -> bool {
    name == syms.floor
        || name == syms.ceil
        || name == syms.to_i
        || name == syms.truncate
        || name == syms.round
}

/// Float -> Int conversion (`x.floor`/`ceil`/`to_i`/`truncate`/`round`) on an F64
/// `x`, producing an i64. `round` is Ruby half-AWAY-from-zero (`trunc(x +
/// copysign(0.5, x))`), NOT cranelift `nearest` (half-even). A branchless RANGE
/// GUARD sets `ovf_var` when the integral value is out of i64 range or NaN/Inf (Ruby
/// returns a bignum / raises there) so the caller deopts to the generic path; the
/// converted value is forced to 0.0 in that case to keep `fcvt_to_sint` from trapping.
fn emit_float_to_int(
    fb: &mut FunctionBuilder,
    ovf_var: Variable,
    name: SymId,
    syms: &JitSyms,
    x: ClValue,
) -> Option<ClValue> {
    // Adjust to the integral F64 the op rounds to; `fcvt_to_sint` then truncates
    // toward zero (exact for the already-integral floor/ceil/round results).
    let adj = if name == syms.to_i || name == syms.truncate {
        x
    } else if name == syms.floor {
        fb.ins().floor(x)
    } else if name == syms.ceil {
        fb.ins().ceil(x)
    } else if name == syms.round {
        let half = fb.ins().f64const(0.5);
        let signed_half = fb.ins().fcopysign(half, x);
        fb.ins().fadd(x, signed_half)
    } else {
        return None;
    };
    // In range iff -2^63 <= adj < 2^63 (ordered fcmp -> NaN/Inf fail -> deopt).
    let lo = fb.ins().f64const(-9223372036854775808.0); // -2^63
    let hi = fb.ins().f64const(9223372036854775808.0); // 2^63
    let ge = fb.ins().fcmp(FloatCC::GreaterThanOrEqual, adj, lo);
    let lt = fb.ins().fcmp(FloatCC::LessThan, adj, hi);
    let ok = fb.ins().band(ge, lt);
    let bad = fb.ins().bxor_imm(ok, 1);
    let cur = fb.use_var(ovf_var);
    let nv = fb.ins().bor(cur, bad);
    fb.def_var(ovf_var, nv);
    let zero = fb.ins().f64const(0.0);
    let safe = fb.ins().select(ok, adj, zero);
    Some(fb.ins().fcvt_to_sint(types::I64, safe))
}

fn emit_binop_float(
    fb: &mut FunctionBuilder,
    k: BinOpKind,
    a: ClValue,
    b: ClValue,
    stack: &mut Vec<(ClValue, Kind)>,
) -> Option<()> {
    // Comparisons -> Bool. The ORDERED variants (LessThan/.../Equal) are false when
    // either operand is NaN, and NotEqual is true for NaN — exactly Ruby's float
    // comparison semantics (`NaN < x`, `NaN == x` are false; `NaN != x` is true). So
    // a Float predicate (`select`/`count { |x| x > 2.0 }`) needs no NaN special-case
    // (unlike `min_by`, which must RAISE on a NaN key).
    let fcc = match k {
        BinOpKind::Lt => Some(FloatCC::LessThan),
        BinOpKind::Le => Some(FloatCC::LessThanOrEqual),
        BinOpKind::Gt => Some(FloatCC::GreaterThan),
        BinOpKind::Ge => Some(FloatCC::GreaterThanOrEqual),
        BinOpKind::Eq => Some(FloatCC::Equal),
        BinOpKind::Ne => Some(FloatCC::NotEqual),
        _ => None,
    };
    if let Some(cc) = fcc {
        let r = fb.ins().fcmp(cc, a, b);
        stack.push((r, Kind::Bool));
        return Some(());
    }
    let r = match k {
        BinOpKind::Add => fb.ins().fadd(a, b),
        BinOpKind::Sub => fb.ins().fsub(a, b),
        BinOpKind::Mul => fb.ins().fmul(a, b),
        BinOpKind::Div => fb.ins().fdiv(a, b),
        _ => return None, // `%` (fmod) needs a libcall — decline
    };
    stack.push((r, Kind::Float));
    Some(())
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
    // Pure unary Int primitives — lowered inline so a block like `{ |x| x.even? }`
    // or a key `{ |x| x.abs }` compiles instead of declining on the method call.
    pub abs: SymId,
    pub even_p: SymId,
    pub odd_p: SymId,
    pub zero_p: SymId,
    pub positive_p: SymId,
    pub negative_p: SymId,
    // Float -> Int conversions (a Float receiver -> `fcvt_to_sint` with a range-guard
    // deopt; an Int receiver -> identity). `round` is Ruby half-away-from-zero.
    pub floor: SymId,
    pub ceil: SymId,
    pub to_i: SymId,
    pub truncate: SymId,
    pub round: SymId,
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

/// Native primitive: push `Value::Float(f64::from_bits(bits))` onto `Array objid`.
/// The Float `select`/`reject`/`find` drivers push the matching float ELEMENT (its
/// f64 bits in the i64 ABI). Caller reserved capacity, so no realloc — GC-free.
///
/// # Safety
/// `vm` valid; `objid` a live pinned Array with reserved capacity.
pub(crate) unsafe extern "C" fn jit_array_push_float(vm: *mut crate::vm::Vm, objid: i64, bits: i64) {
    let vm = unsafe { &mut *vm };
    let id = crate::value::ObjId(objid as u32);
    vm.heap.array_mut(id).push(Value::Float(f64::from_bits(bits as u64)));
}

/// Native primitive: allocate a fresh empty Hash, return its ObjId as i64. Used by
/// the `group_by` driver to build the result Hash. Like `jit_array_new`, the alloc
/// does NOT run GC, so the loop stays GC-free.
///
/// # Safety
/// `vm` valid.
pub(crate) unsafe extern "C" fn jit_hash_new(vm: *mut crate::vm::Vm) -> i64 {
    let vm = unsafe { &mut *vm };
    let id = vm
        .heap
        .alloc(crate::heap::HeapObj::Hash(crate::heap::HashObj::with_pairs(Vec::new())));
    id.0 as i64
}

/// Native primitive for `group_by`: append `Value::Int(elem)` to the bucket Array
/// at integer `key` in Hash `hash_objid`, creating the bucket (a fresh Array) on
/// first sight of the key. First-appearance key order matches CRuby. The scratch
/// Hash keeps `index == None` (never triggered here), so the linear `position`
/// scan stays consistent and a later lookup rebuilds the index lazily from `pairs`.
///
/// # Safety
/// `vm` valid; `hash_objid` a live Hash from `jit_hash_new` in the same loop; all
/// allocations are GC-free, so the in-flight scratch Hash + buckets stay alive.
pub(crate) unsafe extern "C" fn jit_group_push(
    vm: *mut crate::vm::Vm,
    hash_objid: i64,
    key: i64,
    elem: i64,
) {
    let vm = unsafe { &mut *vm };
    let hid = crate::value::ObjId(hash_objid as u32);
    let pos = vm
        .heap
        .hash(hid)
        .iter()
        .position(|(k, _)| matches!(k, Value::Int(n) if *n == key));
    let arr_id = match pos {
        Some(p) => match vm.heap.hash(hid)[p].1 {
            Value::Array(a) => a,
            _ => return, // bucket is always an Array; defensive
        },
        None => {
            let new_arr = vm
                .heap
                .alloc(crate::heap::HeapObj::Array(Vec::<Value>::new().into()));
            vm.heap
                .hash_mut(hid)
                .push((Value::Int(key), Value::Array(new_arr)));
            new_arr
        }
    };
    vm.heap.array_mut(arr_id).push(Value::Int(elem));
}

/// Like [`jit_group_push`] but the bucketed ELEMENT is a Float (`elem` carries its
/// f64 bits) while the KEY stays an Int — for `floats.group_by { |x| x.floor }` and
/// friends, where the key is a Float->Int conversion but the grouped values are the
/// original Floats. Int keys keep the bucket match exact (no -0.0/NaN Float-key
/// subtlety; that variant is deferred).
///
/// # Safety
/// `vm` valid; `hash_objid` a live pinned result Hash; buckets are always Arrays.
pub(crate) unsafe extern "C" fn jit_group_push_floatelem(
    vm: *mut crate::vm::Vm,
    hash_objid: i64,
    key: i64,
    elem: i64,
) {
    let vm = unsafe { &mut *vm };
    let hid = crate::value::ObjId(hash_objid as u32);
    let pos = vm
        .heap
        .hash(hid)
        .iter()
        .position(|(k, _)| matches!(k, Value::Int(n) if *n == key));
    let arr_id = match pos {
        Some(p) => match vm.heap.hash(hid)[p].1 {
            Value::Array(a) => a,
            _ => return,
        },
        None => {
            let new_arr = vm
                .heap
                .alloc(crate::heap::HeapObj::Array(Vec::<Value>::new().into()));
            vm.heap
                .hash_mut(hid)
                .push((Value::Int(key), Value::Array(new_arr)));
            new_arr
        }
    };
    vm.heap
        .array_mut(arr_id)
        .push(Value::Float(f64::from_bits(elem as u64)));
}

/// Like [`jit_group_push`] but BOTH the bucket KEY and the grouped ELEMENT are Floats
/// (their f64 bits in `key`/`elem`) — for `floats.group_by { |x| x * 2.0 }` and other
/// Float-keyed groupings. The bucket match replicates `Value::ruby_eql`'s Float arm
/// exactly (NaN compares by bits so distinct-NaN keys never collide; every other Float
/// by `==`, so -0.0 and 0.0 share a bucket — matching CRuby Hash-key semantics).
///
/// # Safety
/// `vm` valid; `hash_objid` a live pinned result Hash; buckets are always Arrays.
pub(crate) unsafe extern "C" fn jit_group_push_floatkey(
    vm: *mut crate::vm::Vm,
    hash_objid: i64,
    key: i64,
    elem: i64,
) {
    let vm = unsafe { &mut *vm };
    let hid = crate::value::ObjId(hash_objid as u32);
    let key_f = f64::from_bits(key as u64);
    let pos = vm.heap.hash(hid).iter().position(|(k, _)| match k {
        Value::Float(a) => {
            if a.is_nan() && key_f.is_nan() {
                a.to_bits() == key as u64
            } else {
                *a == key_f
            }
        }
        _ => false,
    });
    let arr_id = match pos {
        Some(p) => match vm.heap.hash(hid)[p].1 {
            Value::Array(a) => a,
            _ => return,
        },
        None => {
            let new_arr = vm
                .heap
                .alloc(crate::heap::HeapObj::Array(Vec::<Value>::new().into()));
            vm.heap
                .hash_mut(hid)
                .push((Value::Float(key_f), Value::Array(new_arr)));
            new_arr
        }
    };
    vm.heap
        .array_mut(arr_id)
        .push(Value::Float(f64::from_bits(elem as u64)));
}

/// Native primitive: length of Array `objid` as i64. Read once at the top of a
/// whole-loop driver (`compile_native_sum_loop`); the loop is GC-free so the
/// length stays valid for the duration.
///
/// # Safety
/// `vm` valid; `objid` a live Array (pinned by the caller across the loop).
pub(crate) unsafe extern "C" fn jit_array_len(vm: *const crate::vm::Vm, objid: i64) -> i64 {
    let vm = unsafe { &*vm };
    vm.heap.array(crate::value::ObjId(objid as u32)).len() as i64
}

/// Native primitive: `Array objid[i]` as i64 if it is an `Int`, else deopt
/// (`ovf=1`). The whole-loop driver keeps `0 <= i < len`, but `get` is used (not
/// raw index) so an out-of-range `i` deopts rather than panics.
///
/// # Safety
/// `vm` valid; `objid` a live pinned Array.
pub(crate) unsafe extern "C" fn jit_array_elem_int(
    vm: *const crate::vm::Vm,
    objid: i64,
    i: i64,
) -> NRet {
    let vm = unsafe { &*vm };
    let arr = vm.heap.array(crate::value::ObjId(objid as u32));
    match arr.get(i as usize) {
        Some(Value::Int(n)) => NRet { res: *n, ovf: 0 },
        _ => NRet { res: 0, ovf: 1 },
    }
}

/// Native primitive: `Array objid[i]` as f64 BITS (`f64::to_bits` in `res`) if it
/// is a `Float`, else deopt (`ovf=1`). The Float-element drivers (`sum`) read the
/// element this way; a non-Float element (Int, etc.) deopts to the generic walk
/// rather than silently coercing.
///
/// # Safety
/// `vm` valid; `objid` a live pinned Array.
pub(crate) unsafe extern "C" fn jit_array_elem_float(
    vm: *const crate::vm::Vm,
    objid: i64,
    i: i64,
) -> NRet {
    let vm = unsafe { &*vm };
    let arr = vm.heap.array(crate::value::ObjId(objid as u32));
    match arr.get(i as usize) {
        Some(Value::Float(f)) => NRet {
            res: f.to_bits() as i64,
            ovf: 0,
        },
        _ => NRet { res: 0, ovf: 1 },
    }
}

/// Native primitive: store `Value::Int(val)` at `Array objid[i]`. Used by the
/// whole-loop `map` driver to fill a result array that the caller pre-sized to
/// the input length (so this never grows/reallocs — no GC, no element move).
///
/// # Safety
/// `vm` valid; `objid` a live pinned Array with `len > i >= 0`.
pub(crate) unsafe extern "C" fn jit_array_set_int(
    vm: *mut crate::vm::Vm,
    objid: i64,
    i: i64,
    val: i64,
) {
    let vm = unsafe { &mut *vm };
    let arr = vm.heap.array_mut(crate::value::ObjId(objid as u32));
    if let Some(slot) = arr.get_mut(i as usize) {
        *slot = Value::Int(val);
    }
}

/// Native primitive: store `Value::Float(f64::from_bits(bits))` at `Array objid[i]`.
/// The Float `map` driver's per-element write (`bits` is the block's f64 result in
/// the i64 ABI). Caller pre-sizes the array, so this never grows it — no GC.
///
/// # Safety
/// `vm` valid; `objid` a live pinned Array sized `> i`.
pub(crate) unsafe extern "C" fn jit_array_set_float(
    vm: *mut crate::vm::Vm,
    objid: i64,
    i: i64,
    bits: i64,
) {
    let vm = unsafe { &mut *vm };
    let arr = vm.heap.array_mut(crate::value::ObjId(objid as u32));
    if let Some(slot) = arr.get_mut(i as usize) {
        *slot = Value::Float(f64::from_bits(bits as u64));
    }
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
    // Same shape gate as `compile`: the value-JIT ABI passes exactly one arg, so
    // the method must take exactly one required positional. Without this, a
    // 0-param getter (`def x; @x; end`, body `[LoadIvar, Return]`) compiles here
    // and the 1-arg dispatch hook serves a WRONG-arity call (`obj.x(1)`) from it,
    // swallowing the ArgumentError the interpreter must raise. The plain getter
    // is served at argc==0 by the `getter_ivar` fast path — never through here.
    if proto.n_required_positional != 1
        || proto.params.len() != 1
        || proto.rest_param.is_some()
        || !proto.kw_param_defaults.is_empty()
    {
        return None;
    }
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
