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

### Phase 2 — JIT-addressable class-pointer table + `#[repr(C)]` view
A `class_ptrs: Vec<usize>` indexed by `oid` (the value `class_ptr_of` returns: singleton
class if present, else class; `0` for non-Instance), maintained at the `alloc` chokepoint +
the 3 `singleton_class` set sites (a dead slot's stale entry is never read — `class_ptr_of`
on a dead oid already panics). Exposed through a `#[repr(C)]` `JitObjView { class_ptrs:
*const usize, len: usize }` at a **stable heap address** (`Box`) the JIT bakes once and
loads the live base through (the `Vec` may reallocate; the view's address does not).
Dogfooded by routing `class_ptr_of` through the table behind a STRESS-mode debug-assert that
it equals the slab-derived class — proving the sync is correct before any codegen relies on
it.

### Phase 3 — inline the class guard in codegen
Replace the `class_ptr_of` primitive call inside the obj-call PICs (`jit_obj_call`,
`jit_inst_obj_call`, `jit_value_is_a`) with inline Cranelift loads: deref the recv `Value`
→ `oid` (offset 8, Phase 1) → `view.class_ptrs[oid]` (Phase 2). **Broad win** — every PIC
guard in every compiled method, not just treesum (`bench_walk`, OO, AR all gain).

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
