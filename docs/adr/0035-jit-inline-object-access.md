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

### Phase 4 — JIT-readable ivar array (✅ done; SIMPLER than a storage rewrite)
The original plan (flat `Box<[Value]>` slot storage) hit a wall: the Instance size budget
(≤136B, the `HashObj` ceiling) won't fit an inline-4 flat array without SmallVec's union
trick, and dropping inline-small regresses alloc-heavy workloads (AR). The realization:
**keep SmallVec, just make its element JIT-readable.** Replaced the bare tuple `(SymId,
Value)` with a `#[repr(C)] IvarPair { sym @0, val @8, stride 24 }` and exposed the contiguous
base via `IvarTable::as_ptr_len()` — backed by `SmallVec::as_ptr()`, a STABLE public API, not
the SmallVec's fragile internals. No storage replacement, no `Instance` `#[repr(C)]`, no size
change, inline-small preserved. Contained to IvarTable (its 145 callers untouched — it was
fully encapsulated). offset/size const-asserts pin it. diff_cruby GREEN both builds (967),
zero behaviour change.

### Phase 5 — inline ivar reads in codegen (✅ done; BROAD win, treesum now slab-bound)
Shipped: `jit_self_ivars(self) -> (base, len)` ONE primitive at entry (the ivar array via
`as_ptr_len`), then `emit_ivar_scan` — an inline linear scan of `base[0..len]` for the ivar's
sym — replaces `jit_inst_get_int` (`@v`) and `jit_inst_obj_call` (`@l`/`@r`). For an Int read
(`emit_ivar_int_read`) the i64 payload is loaded + a `tag != 0` Int check; for a receiver, the
scan's address feeds the Phase 3b inline guard + `call_indirect`, with the call SKIPPED on a
scan-miss / non-Object (so a side-effecting callee never double-runs on the deopt-redo). Sound
by construction: the loop bound `i < len` reads only initialized slots; no slot map needed
(self-contained). The `_ =>` materialised-recv guard was factored into `emit_inline_guard_call`,
reused by both arms.

**Result — the BROAD win landed, treesum is now provably slab-bound.** A single-ivar getter in
a loop (`@x` summed, `poc/jit-spike/bench_getter.rb`): jitN **0.145ms vs YJIT 0.186ms — beats
YJIT 1.28×** (the inline read is fast; the per-call `jit_self_ivars` slab lookup AMORTIZES over
the loop). `bench_hammer` (a method's `@b` read now inlines too): 0.50 → 0.47ms. treesum:
0.29 → **0.258ms vs YJIT 0.187ms** — closed 1.55× → 1.38×, but NOT surpassed. A profile shows
the only remaining named frame is `jit_self_ivars`: treesum calls it ONCE PER RECURSIVE `sum()`
(it can't amortize, unlike the getter loop), and that `oid → slab slot → Instance` lookup is
the structural objects-as-oid cost YJIT pays 0 for (its self is a direct pointer). The
inline-ivar mechanism is proven fast (getter beats YJIT); **treesum's residual is purely the
per-call slab lookup.** Correctness: parity interp == JIT == CRuby + STRESS_GC on
`jit_inline_ivar_read` (treesum shape, REVERSE-order ivars [scan matches by sym], missing-ivar
deopt); diff_cruby GREEN both builds (967); debug jit-native scan codegen clean (no desync).

**Full treesum surpass needs Phase 6 — objects as direct pointers** (eliminate the `oid → slab`
indirection so `self` is a pointer and `jit_self_ivars` disappears). That is a far larger
heap/GC rewrite (every `ObjId` becomes a pointer; the slab, the free list, marking, the
`class_ptrs`/view tables all change) — its own ADR, deferred. The broad inline-object-access win
(Phases 3+5: obj-calls and ivar reads beat YJIT) is banked independent of it.

### Phases 4/5 FINAL — FLAT ivar layout: per-class union shapes + slot-indexed storage (✅ shipped 2026-07-02)

The scan-based Phase 4/5 above was superseded by the full storage change it deferred: ivar
access is now a **guarded offset load**, not a scan.

**Shape strategy — per-class UNION table** (`Class::ivar_shape`: `name → slot`, monotonic:
names only ever ADD and a slot, once handed out, never renumbers). Chosen over CRuby-style
shape-transition trees because it needs NO escape-hatch second representation (removal,
out-of-order assignment, and reflection all stay in the one model) and no transition machinery;
the cost — instances assigning in different orders share slot numbering, and per-object holes
for slots an object never assigned — is bounded by the class's own name count (rubocop's Node
family is monomorphic-init, the driving case). Monotonicity is the invalidation story: baked
`(class, slot)` pairs can go class-stale but never slot-stale, so no generation counter exists
anywhere in the design.

**Instance storage** (`IvarTable`): `slots: SmallVec<[Value;4]>` indexed by the class shape
(lazily grown; HOLES hold `Nil`, so the hot read path loads `slots[slot]` raw and undefined
ivars read as nil for free) + `order: SmallVec<[u32;4]>` (per-object ASSIGNMENT order — CRuby's
`instance_variables` contract is per-object, not per-class — doubling as the defined-set for
iteration) + a `bits: u64` O(1) defined-set for the write path (slot ≥ 64 falls back to an
order scan). Same 104-byte footprint as the scan table; `HeapObj` did not grow. The snapshot
image stays NAME-keyed (`(SymId, ValueImage)` in assignment order), so restore re-interns into
the restored class's shape and the image format is numbering-independent.

**The fast paths, one per tier:**
- **Interpreter**: `LoadIvar`/`StoreIvar`/`IncIvar*` ops carry a per-site cid into
  `Vm::ivar_caches` (its OWN id space — `CidGen { call, ivar }` — so neither dense cache vec
  inflates the other; persisted through preamble-cache [RBPC v3] + snapshot [RRS1 v4] +
  `reset()` truncation). Hit = class ptr compare → direct slot load/store.
- **Polymorphism-proofing** (the wall the first cut hit): rubocop visits ~40 Node SUBCLASSES
  through shared methods/getter protos, thrashing any class-keyed cache. Sibling subclasses
  build shapes through the same `initialize` order ⇒ same numbering, so the caches verify by
  CONTENT — `names[slot] == sym`, one indexed borrow-flag-free compare
  (`Class::ivar_shape_name_at`) — and hit across subclasses exactly.
- **Frame-free getter serves** (dispatch's hottest ivar path): a per-proto content-verified
  slot cell (`Proto::getter_slot`, runtime-only) — no probe on the serve.
- **jit-native**: ONE fused entry primitive `jit_self_ivars(vm, self, frame_block)` returns
  the slot-array base+len AND resolves every baked ivar sym behind a per-frame class memo
  (same class as the last frame ⇒ zero resolution work). Compiled reads are a branch-free
  bounds-check + `base + slot*16` load on entry-loaded SSA slots — slot resolution is
  loop-invariant, so per-READ guards (first cut, measured +39% on `bench_getter`) were hoisted
  out entirely. The Phase-5 inline scan loop, `jit_ivar_slot`, and the per-site cells are gone.
- **tier-2 helpers**: borrow-free shape scan (`ivar_slot_lookup_fast` — the class's `names`
  vec is shared by every instance, i.e. always hot) — measured faster than touching a
  per-site cache line per op.

**Measured** (Apple Silicon, interleaved A/B vs the unmodified-HEAD baseline binary):
- Micro, 6-ivar object, tier2: deep ivar read **5.07 → 2.18 ns** (depth-INDEPENDENT; the old
  scan paid per slot depth); first-ivar read 1.50 → 2.07 ns (the one shape that regresses —
  the old scan's best case); CRuby 13.4 / YJIT 14.0 on the same loop.
- 40-subclass polymorphic getter micro: interp −7%/−10%; tier2 par.
- `bench_getter` (ivar read in a native loop): 0.164ms — still beats YJIT (0.194); `bench_hammer`
  0.60 vs baseline 0.50 (+20%, the per-frame entry cost on 1-read-per-call bodies — still beats
  YJIT 0.70). The tradeoff is explicit: ~1ns/frame of entry memo work buys depth-independence
  and polymorphism-proofness.
- walkonly big1 (the rubocop cop walk): par within noise both modes (LoadIvar is ~3.7% of walk
  ops; the interpreter deltas sit inside the ±3% machine noise). rubocop f1/big1/batch20 output
  byte-identical to baseline, tier on and off; RSS on the big1 run +0.3–0.5%.
- fib canary unchanged (8.1ms, beats YJIT's 11.0).

**Gates**: diff_cruby 1066/0 in all four configs (default / JIT_NATIVE / TIER2 /
TIER2+THRESHOLD); STRESS_GC sweep incl. the new `ivar_reflection_battery` fixture (order,
defined?-vs-nil, remove/re-add, frozen, dup/clone sharing, >8-name shapes, sibling-subclass
order independence) at CRuby parity; a new snapshot roundtrip test
(`restore_flat_ivars_order_and_sharing`) proves order + shared refs + holes survive
image→restore; rubocop image save→load byte-identical to cold.

**Phase 6 (if any)**: the remaining ivar-side costs are (a) the per-frame entry memo on
1-read-per-call JIT bodies (`bench_hammer`'s ~1ns — fusable further only by shrinking the
primitive itself) and (b) the `oid → slab` indirection ADR 0036 already rejected re-litigating.
A future direction with better leverage than either: subtree-SHARED shapes (a subclass adopting
its superclass's table by reference) would turn the content-verify into a plain pointer compare
and shrink shape memory, at the cost of union-bloat control (a root-class heuristic). Not
scheduled — the current design's caches already absorb the polymorphism it would address.

#### Phase 5 original plan (superseded — kept for context)
Per-node, treesum pays ~6 JIT↔primitive boundaries (`jit_self_inst` + `jit_inst_get_int` for
`@v` + 2× `jit_inst_obj_call` for `@l`/`@r`, each of which also calls its callee). Plan to
halve it to 3:
- `jit_self_ivars(self) -> (base, len)` ONE primitive at entry (the ivar array base via
  `as_ptr_len`), replacing `jit_self_inst`.
- Inline ivar read for `@v`/`@l`/`@r`: read `base[slot]` (`IvarPair`, Phase 4 offsets) and
  verify `sym` (deopt on mismatch). The slot comes from a **per-class slot map**
  (`Class.ivar_slots: SymId→u16`, recorded at the `SetIvar` opcode — the receiver's class is
  there) which the JIT reads at compile time (it has `recv_cls`); the runtime `sym` verify
  makes it sound regardless of order. (Alternative: an inline scan loop, no slot map but
  heavier codegen — the slot-map+bake path has the simpler, lower-risk runtime codegen.)
- For `@l.sum`/`@r.sum`: the inline ivar read yields the recv pointer, then Phase 3b's inline
  class guard + `call_indirect`. So `jit_inst_obj_call` retires on the hot path.
Result target: **treesum beats YJIT**, plus a broad speedup for every self-ivar read (getters,
AR attributes — like Phase 3b broadened past treesum). The most intricate codegen of the
investment; done as its own validated step (diff_cruby both + STRESS_GC + A/B measure).

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
