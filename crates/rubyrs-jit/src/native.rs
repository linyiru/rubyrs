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
