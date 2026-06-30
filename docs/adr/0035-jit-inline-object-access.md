# ADR 0035 — JIT-inline object access (the object-header investment)

Status: **Accepted, in progress** (2026-06-30)
Follows: ADR 0034 (JIT-first surpass YJIT), ADR 0033 (lean VM core)

## Context

ADR 0034 got the native Cranelift JIT past YJIT on every realistic dispatch shape
(`fib`, the OO north-star, `bench_walk`, the `.each` cop-body walk). One synthetic shape
still trails: `bench_treesum` (`@v + @l.sum(d-1) + @r.sum(d-1)`), at ~1.5× YJIT.

The feasibility PoC `poc/jit-spike/treesum_rep.rs` (committed 8a9f17b4) proved **why** and
**that the fix is worth it**. Running the treesum recursion over three object
representations on a contiguous arena (locality held constant):

| representation | per_node | note |
|---|---|---|
| slab+enum, every access a primitive call | ~5.9 ns | pre-caching rubyrs |
| self-`Instance` cached (shipped) | ~6.0 ns | +4.5% vs above — **reproduces** the real ~4% |
| **object-header, accesses INLINED** | **0.92 ns** | object = ptr, class word + fixed ivar offsets |

The transferable figure is the per-node **access overhead a header rep removes: ~5.2 ns**,
which **exceeds the real jitN→YJIT gap of 3.05 ns/node** (jitN 8.85, YJIT 5.80). So a header
rep would close treesum's gap and likely beat YJIT.

**The cost is the per-access PRIMITIVE-CALL boundary, not the slab index.** The JIT cannot
inline `oid → slots[oid] → Slot::Live → HeapObj::Instance → Instance{class, ivars}` into
Cranelift (none of those types has a pinned layout), so it emits an extern-C primitive call
per ivar/class read (`jit_inst_get_int`, the class guard via `class_ptr_of`). YJIT/CRuby do
each as a single inlinable load: the class is a header word, ivars sit at fixed offsets.

The lever is therefore **making object access INLINABLE** — pinned, `#[repr(C)]`-known
layouts the Cranelift codegen can load through directly, behind a stable-addressed view.

## Decision

Invest in JIT-inline object access, **phased so each phase ships independently and is
validatable on its own** (diff_cruby GREEN both builds + STRESS_GC, no baseline regression).
Everything is gated behind `feature = "jit-native"` where it adds cost — the default build
pays nothing.

### Phase 1 — pin `Value`'s memory layout (FOUNDATION, ✅ done)
`Value` is a plain Rust enum with no `#[repr]`, so the offset of the `Object(ObjId)` payload
is compiler-chosen and unstable. The JIT holds a `Kind::Object` as a `*const Value`; to read
its `oid` inline it must load the `u32` at a known offset. Added `#[repr(u8)]` to `Value`,
which gives a defined layout: a `u8` tag at offset 0, then each variant's fields at their
natural alignment. An ObjId variant is `{u8 tag, u32 oid}`, so **`oid` is at offset 4**
(`OID_OFFSET`); a `{u8 tag, i64}` variant keeps its payload at offset 8, and the widest
variant makes `size_of::<Value>() == 16`. Pinned with compile-time `size`/`align` asserts and
a contract test (`value_layout_contract`) that round-trips `ObjId(n)` through a raw `u32` read
at offset 4 (and an `i64` at offset 8). **Zero behaviour change** — purely a layout annotation
+ its contract test; diff_cruby GREEN both builds. This unblocks every later inline-load phase.
(Per-variant tag VALUES are NOT stable — they shift with cfg-gated variants
[bignum/regex/rational] — so a phase wanting an inline discriminant CHECK must read the live
tag at runtime, not bake a constant. Only `OID_OFFSET` is stable. The kind tracking already
guarantees the variant, so the hot path needs no tag check — the same trust the existing
primitives' deopt net provides.)

### Phase 2 — JIT class-pointer table (✅ done; the `#[repr(C)]` view moves to Phase 3)
A `class_ptrs: Vec<usize>` PARALLEL to `slots` (gated `feature = "jit-native"`), holding the
value `class_ptr_of` returns (singleton class if present, else class; `0` for objects with no
dispatchable class, and for a `Fiber` before its class is cached → caller falls back to the
slab). Maintained at the **two** points that set an object's effective class: `alloc` (via a
`class_ptr_of_obj` helper, computed before the object is moved into its slot) and
`ensure_singleton_class`. The 3rd/4th `singleton_class` set sites turned out to be HASH
singletons — `class_ptr_of` returns `None` for Hash, so they don't affect the table.
`alloc` is the sole `slots` growth point, so the table stays length-synced; a swept slot's
entry is stale but never read (`get` panics on a dead oid) and is overwritten on reuse.
Dogfooded: `class_ptr_of` now serves from the table (returning the slab walk only for `0`
entries), behind a `debug_assert` that a nonzero entry equals the slab-derived class —
proving the sync before codegen relies on it. **Validated**: the full diff_cruby suite in a
DEBUG jit-native build (assert live) passed all but 2 deep-recursion fixtures, which fail on
a pre-existing debug-only `SystemStackError` (confirmed identical in a default debug build —
unrelated to this change); plus a STRESS_GC churn test (20 000 allocs across two classes +
singleton additions, forcing slot reuse) at exact CRuby parity with no desync. Release
diff_cruby GREEN both builds (966). The `#[repr(C)] JitObjView` (a stable-addressed `Box`
the JIT bakes + loads the live `class_ptrs` base through) is deferred to Phase 3, where the
codegen that needs it lands.

### Phase 3 — inline the class guard in codegen (✅ done for the generic obj-call; bool/is_a/ivar-recv next)
Replaced the `jit_obj_call` primitive call for a materialized-recv obj-call (the `_ =>` arm)
with an inline class-guard fast path: load `view.class_ptrs` (offset 0 of the baked
`JitObjView`), extract `oid` from the recv `Value` (`u32` at offset 4, Phase 1), load
`class_ptrs[oid]` (Phase 2), compare to the PIC's cached class, and on a hit
`call_indirect` the cached callee directly — skipping the primitive frame + its
`class_ptr_of`. Cold / class-miss / non-Instance recv falls to the existing primitive
(compile + cache + deopt), so correctness is unchanged. Threaded the baked view address
through `JitSyms` (one builder, no per-call-site edits). `view_addr == 0` disables it.

**Measured** (A/B, isolated generic obj-call in a native loop, `poc/jit-spike/bench_hammer.rb`):
fast path OFF 0.82ms, ON **0.50ms — ~1.65×**; ON beats YJIT (0.67ms) by 1.35× where OFF
trailed it. So the inline guard turns a YJIT-loss into a YJIT-win on this shape, confirming
the PoC's thesis (the primitive-CALL boundary, not the slab index, was the cost) in the real
JIT. Note `bench_b4` was NOT the vehicle — `objs.sum { |o| o.m }` uses the B4 whole-loop
compiler, which already bakes its guard; the generic `recv.method(arg)` arm needed a dedicated
bench. Correctness: parity interp == JIT == CRuby + STRESS_GC on `jit_inline_objcall_guard`
(monomorphic fast path, POLYMORPHIC same-site cache-miss → slow-path recompile, native-loop
hammer); diff_cruby GREEN both builds (967); debug jit-native obj-call fixtures clean (no
desync). **Still to inline** (same mechanism): `jit_obj_call_bool` (predicates — `bench_walk`),
`jit_value_is_a` (`is_a?`), and — once Phase 5 inlines the ivar read for the receiver —
`jit_inst_obj_call` (treesum's `@l.sum`). This proves the inline-load codegen Phase 5 reuses.

### Phase 4 — per-class ivar layout (the header proper)
Assign each ivar a fixed slot index per class (a `HashMap<SymId, u16>` on `Class`, filled as
ivars are first assigned), and store an Instance's ivars in a flat `Box<[Value]>` indexed by
slot instead of the `SmallVec<[(SymId, Value); 4]>` scan. The class records its slot map; the
JIT, monomorphic on the guarded class, knows the slot index at compile time.

### Phase 5 — inline ivar reads in codegen
Replace `jit_inst_get_int` / the ivar fetch in `jit_inst_obj_call` with an inline load:
`Instance.ivars_ptr[slot]` at the compile-time slot index. Retire the ivar primitives on the
hot path. **treesum surpass.**

## Consequences

- **Risk is front-loaded into reversible, independently-validated steps.** Phase 1 is a
  layout annotation. Phase 2 adds a table dogfooded under assertion before anything trusts
  it. Only Phase 3+ change codegen, and each is measured against YJIT on its shape.
- The `#[repr(u8)]` on `Value` is a permanent contract; the `size`/offset asserts guard it.
- Phase 4 changes the ivar storage representation — the most invasive step (it touches GC
  marking, ivar get/set, `instance_variables`, marshal). It is deferred behind Phases 1–3
  (which already deliver the broad class-guard win) so the big change lands last, on a proven
  foundation.
- Not coupled to the rubocop product timeline — this is the perf investment for
  object-access-heavy workloads, pursued because the PoC de-risked it, independently of
  shipping rubocop.
