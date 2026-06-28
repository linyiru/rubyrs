//! Native (Cranelift) backend — ADR 0030 finding #4 spike.
//! Proves the seam: compile a function to machine code + call it.

use cranelift_codegen::ir::{types, AbiParam, InstBuilder};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module};

/// Compile `fn() -> i64 { return v }` to native code and return a callable
/// pointer. Smoke test that Cranelift JIT works in this workspace.
pub fn jit_const(v: i64) -> extern "C" fn() -> i64 {
    let builder = JITBuilder::new(cranelift_module::default_libcall_names()).unwrap();
    let mut module = JITModule::new(builder);
    let mut ctx = module.make_context();
    let mut sig = module.make_signature();
    sig.returns.push(AbiParam::new(types::I64));
    ctx.func.signature = sig.clone();
    let fid = module.declare_function("k", Linkage::Export, &sig).unwrap();
    let mut fbctx = FunctionBuilderContext::new();
    {
        let mut fb = FunctionBuilder::new(&mut ctx.func, &mut fbctx);
        let b = fb.create_block();
        fb.switch_to_block(b);
        fb.seal_block(b);
        let cv = fb.ins().iconst(types::I64, v);
        fb.ins().return_(&[cv]);
        fb.finalize();
    }
    module.define_function(fid, &mut ctx).unwrap();
    module.clear_context(&mut ctx);
    module.finalize_definitions().unwrap();
    let code = module.get_finalized_function(fid);
    unsafe { std::mem::transmute::<_, extern "C" fn() -> i64>(code) }
}

/// A backend-agnostic stack-IR op for a 1-parameter integer method. rubyrs
/// lowers an eligible `Proto` (only param reads, int constants, +/-/* on
/// ints) into a `Vec<IntOp>`; this module turns it into native code. Keeps
/// the crate VM-free (no `Op` dependency).
#[derive(Clone, Copy, Debug)]
pub enum IntOp {
    /// Push the method's sole parameter `x`.
    Param,
    /// Push an integer literal.
    Const(i64),
    Add,
    Sub,
    Mul,
}

/// Native return: `result` + `overflow` flag, returned by-value. The SysV
/// ABI passes this two-integer struct in two registers, matching Cranelift's
/// two-value `[i64, i8]` return.
#[repr(C)]
pub struct IntRet {
    pub res: i64,
    pub ovf: u8,
}

/// A compiled 1-param integer method: `fn(x) -> (i64, overflow)`. Holds the
/// `JITModule` so the code stays mapped.
pub struct CompiledInt1 {
    _module: JITModule,
    ptr: extern "C" fn(i64) -> IntRet,
}

impl CompiledInt1 {
    /// Call the native code. Returns `None` if any +/-/* overflowed i64
    /// (the caller must deopt to the interpreter, which promotes to Bignum).
    #[inline]
    pub fn call(&self, x: i64) -> Option<i64> {
        let r = (self.ptr)(x);
        if r.ovf == 0 {
            Some(r.res)
        } else {
            None
        }
    }
}

/// Compile a 1-param integer stack-IR to native code. Each +/-/* uses
/// Cranelift's overflow-checking ops; any overflow ORs into the out-flag so
/// the result is *provably identical* to the interpreter (which would
/// promote to Bignum) — the JIT only commits when it's a pure i64 result.
pub fn compile_int1(ops: &[IntOp]) -> CompiledInt1 {
    let builder = JITBuilder::new(cranelift_module::default_libcall_names()).unwrap();
    let mut module = JITModule::new(builder);
    let mut ctx = module.make_context();
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(types::I64)); // x
    sig.returns.push(AbiParam::new(types::I64)); // result
    sig.returns.push(AbiParam::new(types::I8)); // overflow flag
    ctx.func.signature = sig.clone();
    let fid = module.declare_function("int1", Linkage::Export, &sig).unwrap();
    let mut fbctx = FunctionBuilderContext::new();
    {
        let mut fb = FunctionBuilder::new(&mut ctx.func, &mut fbctx);
        let blk = fb.create_block();
        fb.append_block_params_for_function_params(blk);
        fb.switch_to_block(blk);
        fb.seal_block(blk);
        let x = fb.block_params(blk)[0];
        let mut stack: Vec<cranelift_codegen::ir::Value> = Vec::new();
        let mut ovf_acc = fb.ins().iconst(types::I8, 0);
        for op in ops {
            match op {
                IntOp::Param => stack.push(x),
                IntOp::Const(c) => {
                    let v = fb.ins().iconst(types::I64, *c);
                    stack.push(v);
                }
                IntOp::Add | IntOp::Sub | IntOp::Mul => {
                    let b = stack.pop().expect("int1 IR stack underflow");
                    let a = stack.pop().expect("int1 IR stack underflow");
                    let (res, of) = match op {
                        IntOp::Add => fb.ins().sadd_overflow(a, b),
                        IntOp::Sub => fb.ins().ssub_overflow(a, b),
                        IntOp::Mul => fb.ins().smul_overflow(a, b),
                        _ => unreachable!(),
                    };
                    ovf_acc = fb.ins().bor(ovf_acc, of);
                    stack.push(res);
                }
            }
        }
        let result = stack.pop().expect("int1 IR produced no value");
        fb.ins().return_(&[result, ovf_acc]);
        fb.finalize();
    }
    module.define_function(fid, &mut ctx).unwrap();
    module.clear_context(&mut ctx);
    module.finalize_definitions().unwrap();
    let code = module.get_finalized_function(fid);
    let ptr = unsafe { std::mem::transmute::<_, extern "C" fn(i64) -> IntRet>(code) };
    CompiledInt1 { _module: module, ptr }
}

/// A stand-in for a rubyrs primitive: reads a value THROUGH A POINTER and
/// returns a result. The pointer-passing is the point — it's how the JIT will
/// hand a `Value` to a native primitive without baking the enum layout into
/// codegen (D Layer 3/5: `Hash#[]`, `instance_variable_get`, … as native calls).
extern "C" fn jit_test_helper(p: *const i64) -> i64 {
    unsafe { *p * 10 }
}

/// D Layer 3 PoC: a JIT'd `f(x) = jit_test_helper(&x) + 1`. Proves the external-
/// primitive-call mechanism — register the helper's address as a symbol, import
/// it, spill `x` to a stack slot, pass its ADDRESS to the helper, use the
/// result. This is the seam through which a value-representation JIT calls
/// rubyrs's string/hash/object primitives.
pub fn compile_with_helper() -> extern "C" fn(i64) -> i64 {
    use cranelift_codegen::ir::{StackSlotData, StackSlotKind};
    let mut builder = JITBuilder::new(cranelift_module::default_libcall_names()).unwrap();
    builder.symbol("jit_test_helper", jit_test_helper as *const u8);
    let mut module = JITModule::new(builder);
    let ptr_ty = module.target_config().pointer_type();
    let mut ctx = module.make_context();
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I64));
    ctx.func.signature = sig.clone();
    let fid = module.declare_function("f", Linkage::Export, &sig).unwrap();
    // Import the external primitive: fn(*const i64) -> i64.
    let mut helper_sig = module.make_signature();
    helper_sig.params.push(AbiParam::new(ptr_ty));
    helper_sig.returns.push(AbiParam::new(types::I64));
    let helper_id = module
        .declare_function("jit_test_helper", Linkage::Import, &helper_sig)
        .unwrap();
    let mut fbctx = FunctionBuilderContext::new();
    {
        let mut fb = FunctionBuilder::new(&mut ctx.func, &mut fbctx);
        let helper_ref = module.declare_func_in_func(helper_id, fb.func);
        let b = fb.create_block();
        fb.append_block_params_for_function_params(b);
        fb.switch_to_block(b);
        fb.seal_block(b);
        let x = fb.block_params(b)[0];
        // Spill x to a stack slot and pass its address (the Value-by-pointer seam).
        let slot = fb.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 8, 3));
        fb.ins().stack_store(x, slot, 0);
        let p = fb.ins().stack_addr(ptr_ty, slot, 0);
        let inst = fb.ins().call(helper_ref, &[p]);
        let h = fb.inst_results(inst)[0];
        let one = fb.ins().iconst(types::I64, 1);
        let r = fb.ins().iadd(h, one);
        fb.ins().return_(&[r]);
        fb.finalize();
    }
    module.define_function(fid, &mut ctx).unwrap();
    module.clear_context(&mut ctx);
    module.finalize_definitions().unwrap();
    let code = module.get_finalized_function(fid);
    unsafe { std::mem::transmute::<_, extern "C" fn(i64) -> i64>(code) }
}

/// Compile a SELF-CONTAINED integer loop method to native code — the first
/// shape that makes a fair, end-to-end comparison with YJIT possible (the
/// whole computation is the method body; one call, the loop runs native in
/// both engines, so call overhead is amortised away).
///
/// Compiles exactly:
/// ```ruby
/// def f(n)
///   s = 0; i = 0
///   while i < n
///     s = s ^ (i*i + i*7 + 13)   # XOR: bounded, no overflow, not DCE-able
///     i = i + 1
///   end
///   s
/// end
/// ```
/// Multiplies/adds are overflow-checked (deopt flag); XOR and the loop
/// counter can't overflow for the benchmarked `n`.
pub fn compile_poly_loop() -> CompiledInt1 {
    use cranelift_codegen::ir::condcodes::IntCC;
    let builder = JITBuilder::new(cranelift_module::default_libcall_names()).unwrap();
    let mut module = JITModule::new(builder);
    let mut ctx = module.make_context();
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(types::I64)); // n
    sig.returns.push(AbiParam::new(types::I64)); // s
    sig.returns.push(AbiParam::new(types::I8)); // overflow
    ctx.func.signature = sig.clone();
    let fid = module.declare_function("polyloop", Linkage::Export, &sig).unwrap();
    let mut fbctx = FunctionBuilderContext::new();
    {
        let mut fb = FunctionBuilder::new(&mut ctx.func, &mut fbctx);
        let entry = fb.create_block();
        fb.append_block_params_for_function_params(entry);
        let header = fb.create_block();
        fb.append_block_param(header, types::I64); // s
        fb.append_block_param(header, types::I64); // i
        fb.append_block_param(header, types::I8); // ovf
        let body = fb.create_block();
        fb.append_block_param(body, types::I64);
        fb.append_block_param(body, types::I64);
        fb.append_block_param(body, types::I8);
        let exit = fb.create_block();
        fb.append_block_param(exit, types::I64); // s
        fb.append_block_param(exit, types::I8); // ovf

        // entry: s=0, i=0, ovf=0 → header
        fb.switch_to_block(entry);
        let n = fb.block_params(entry)[0];
        let z64 = fb.ins().iconst(types::I64, 0);
        let z8 = fb.ins().iconst(types::I8, 0);
        fb.ins().jump(header, &[z64.into(), z64.into(), z8.into()]);
        fb.seal_block(entry);

        // header: while i < n
        fb.switch_to_block(header);
        let (hs, hi, hovf) = {
            let p = fb.block_params(header);
            (p[0], p[1], p[2])
        };
        let cond = fb.ins().icmp(IntCC::SignedLessThan, hi, n);
        fb.ins()
            .brif(cond, body, &[hs.into(), hi.into(), hovf.into()], exit, &[hs.into(), hovf.into()]);

        // body: s = s ^ (i*i + i*7 + 13); i = i+1
        fb.switch_to_block(body);
        let (s, i, ovf) = {
            let p = fb.block_params(body);
            (p[0], p[1], p[2])
        };
        let (ii, o1) = fb.ins().smul_overflow(i, i);
        let c7 = fb.ins().iconst(types::I64, 7);
        let (i7, o2) = fb.ins().smul_overflow(i, c7);
        let (p1, o3) = fb.ins().sadd_overflow(ii, i7);
        let c13 = fb.ins().iconst(types::I64, 13);
        let (poly, o4) = fb.ins().sadd_overflow(p1, c13);
        let new_s = fb.ins().bxor(s, poly);
        let one = fb.ins().iconst(types::I64, 1);
        let (new_i, o5) = fb.ins().sadd_overflow(i, one);
        let mut ov = fb.ins().bor(ovf, o1);
        for o in [o2, o3, o4, o5] {
            ov = fb.ins().bor(ov, o);
        }
        fb.ins().jump(header, &[new_s.into(), new_i.into(), ov.into()]);
        fb.seal_block(body);
        fb.seal_block(header);

        // exit: return (s, ovf)
        fb.switch_to_block(exit);
        let (es, eovf) = {
            let p = fb.block_params(exit);
            (p[0], p[1])
        };
        fb.ins().return_(&[es, eovf]);
        fb.seal_block(exit);
        fb.finalize();
    }
    module.define_function(fid, &mut ctx).unwrap();
    module.clear_context(&mut ctx);
    module.finalize_definitions().unwrap();
    let code = module.get_finalized_function(fid);
    let ptr = unsafe { std::mem::transmute::<_, extern "C" fn(i64) -> IntRet>(code) };
    CompiledInt1 { _module: module, ptr }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int1_poly_correct() {
        // poly(x) = x*x + x*7 + 13
        let ir = [
            IntOp::Param, IntOp::Param, IntOp::Mul, // x*x
            IntOp::Param, IntOp::Const(7), IntOp::Mul, // x*7
            IntOp::Add, // x*x + x*7
            IntOp::Const(13), IntOp::Add, // + 13
        ];
        let f = compile_int1(&ir);
        let poly = |x: i64| x * x + x * 7 + 13;
        for x in [-5, 0, 1, 7, 100, 99999] {
            assert_eq!(f.call(x), Some(poly(x)), "x={x}");
        }
        // overflow → None (deopt)
        assert_eq!(f.call(i64::MAX), None);
    }

    #[test]
    fn jit_const_runs() {
        let f = super::jit_const(12345);
        assert_eq!(f(), 12345);
    }

    #[test]
    fn external_helper_call() {
        // jit_test_helper(&x) = x*10 ; compiled f(x) = x*10 + 1
        let f = compile_with_helper();
        assert_eq!(f(5), 51);
        assert_eq!(f(7), 71);
        assert_eq!(f(0), 1);
    }

    #[test]
    fn poly_loop_correct() {
        let f = compile_poly_loop();
        let reference = |n: i64| {
            let mut s = 0i64;
            let mut i = 0i64;
            while i < n {
                s ^= i * i + i * 7 + 13;
                i += 1;
            }
            s
        };
        for n in [0, 1, 10, 1000, 1_000_000] {
            assert_eq!(f.call(n), Some(reference(n)), "n={n}");
        }
    }

    // `cargo test -p rubyrs-jit --release --features native poly_loop_throughput -- --nocapture`
    #[test]
    fn poly_loop_throughput() {
        let f = compile_poly_loop();
        let n: i64 = 1_000_000_000;
        let t = std::time::Instant::now();
        let r = f.call(n);
        let e = t.elapsed().as_secs_f64();
        eprintln!(
            "[native-jit] f(n)=XOR poly(i), f({}): {:.0} M iter/sec  (result={:?})",
            n,
            n as f64 / e / 1e6,
            r
        );
    }

    // `cargo test -p rubyrs-jit --features native int1_poly_throughput -- --nocapture`
    #[test]
    fn int1_poly_throughput() {
        let ir = [
            IntOp::Param, IntOp::Param, IntOp::Mul,
            IntOp::Param, IntOp::Const(7), IntOp::Mul,
            IntOp::Add, IntOp::Const(13), IntOp::Add,
        ];
        let f = compile_int1(&ir);
        let n: u64 = 1_000_000_000;
        let t = std::time::Instant::now();
        let mut acc = 0i64;
        let mut i: i64 = 0;
        while (i as u64) < n {
            acc = acc.wrapping_add(f.call(i & 0xffff).unwrap_or(0));
            i += 1;
        }
        let e = t.elapsed().as_secs_f64();
        eprintln!(
            "[native-jit] poly(x): {:.0} M calls/sec  (acc={})",
            n as f64 / e / 1e6,
            acc
        );
    }
}
