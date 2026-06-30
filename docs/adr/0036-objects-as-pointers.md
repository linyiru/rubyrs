# ADR 0036 — Objects as direct pointers (Phase 6 of the JIT object-header investment)

Status: **REJECTED by feasibility PoC** (2026-06-30) — would not surpass treesum; the gap is
Cranelift codegen quality, not the object representation. Recorded so the question stays
answered.
Follows: ADR 0035 (JIT-inline object access, Phases 1–5 shipped).

## Context

ADR 0035 inlined object access in the native JIT — obj-calls (Phase 3b, beats YJIT 1.35×) and
ivar reads (Phase 5; a getter in a loop beats YJIT 1.28×). The one holdout is `bench_treesum`
(`@v + @l.sum(d-1) + @r.sum(d-1)`): jitN 0.258ms vs YJIT 0.187ms, ~1.38× behind. After Phase 5
the only remaining per-node *primitive* is `jit_self_ivars`, the `oid → slab[oid] → Slot →
HeapObj::Instance` lookup that converts an `ObjId` to its `Instance` (YJIT pays 0 for this —
its `self` is a direct pointer). The hypothesis: represent objects as **direct pointers**
(eliminate the `oid → slab` indirection) so `jit_self_ivars` disappears and treesum surpasses.

This would be the largest change of the whole arc: every `ObjId` becomes a pointer; the slab
`Vec<Slot>`, the free list, GC marking, the `class_ptrs` table + `JitObjView` all change; the
heap can no longer relocate objects on `Vec` growth (pointers must be stable → per-object or
arena allocation). A multi-week rewrite touching the core of the runtime. So — per the
discipline that served ADR 0035 well — a feasibility PoC FIRST.

## The PoC (`poc/jit-spike/objptr_feasibility.rs`) + the real profile

The PoC runs the treesum recursion three ways, all with Phase-5 inline ivar reads + an inline
class guard, differing only in how `self`'s data is reached: `RepA_now` (slab lookup via a
non-inlined call — today), `RepA_inlined` (same walk, inlined), `RepB_directptr` (object is a
pointer, one deref — Phase 6). Per-node: RepA_now 2.08ns, RepA_inlined 1.72ns, RepB 0.97ns —
so a pointer removes ~1.1ns of the ~2.16ns real gap. But the PoC is pure `rustc -O`, whose lean
recursion skeleton inflates the slab's *relative* share; it is not authoritative on absolute
fractions.

**The real profile is.** Sampling the actual jitN treesum run, the hot frames are the
ANONYMOUS Cranelift-generated code (the compiled `sum` body — the `call_indirect` recursion +
arithmetic + inline reads + guards): the two hottest addresses alone are 1622 + 1415 samples.
`jit_self_ivars` is **176 samples — ~5% of the run.** (The earlier "only named frame" read was
an artifact: the JIT code is unnamed, so only the Rust primitive showed a symbol.)

## Decision — REJECTED

**Eliminating the slab lookup would save ~5% of treesum's time; the gap is ~27%.** Phase 6
would leave treesum ~1.3× behind YJIT. The residual is the **Cranelift-generated code quality
vs YJIT's** for this recursion shape — the `call_indirect` recursion, the per-node arithmetic,
the guard sequence — none of which a pointer rewrite touches. A multi-week heap/GC rewrite for
a ~5% move on one synthetic micro is not worth it.

The broad win is already banked: with Phases 1–5, real-world object access — obj-calls and
ivar reads, the overwhelmingly common amortized (loop) case — already beats YJIT. treesum is
the pathological per-recursive-call shape that can't amortize anything, and even there the
remaining gap is codegen, not representation.

## Consequences

- **No objects-as-pointers rewrite.** The `ObjId`/slab representation stays. ADR 0035's gains
  stand on their own.
- If a future workload makes treesum-shape (deep per-call recursion, no amortization) genuinely
  matter, the lever to investigate is **codegen quality** — the `call_indirect` recursion (a
  direct/inlined recursive call instead of a PIC-cached indirect one) and Cranelift opt
  settings — NOT the object representation. That is a separate, smaller, and more promising
  inquiry than this ADR's pointer rewrite.
- Feasibility PoC + profile cost ~an hour and saved a multi-week investment that would not have
  achieved its goal. The pattern (model + measure before committing) is the takeaway.
