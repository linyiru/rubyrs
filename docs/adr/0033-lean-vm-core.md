# ADR 0033 — Lean VM core (ground-up execution engine for CRuby parity)

## Status

**Superseded by [ADR 0034](0034-jit-first-surpass-yjit.md)** (2026-06) — the
lean VM core is re-scoped there as the JIT's value substrate and deopt
baseline rather than a standalone interpreter rewrite. Original status:
Proposed / de-risking in progress.

## Context

rubyrs is ~7× slower than CRuby (and ~10× YJIT) on framework dispatch
(ActiveRecord CRUD: rubyrs 2528ms / CRuby 339ms / YJIT 246ms). A rigorous
investigation (this cycle) established **why** and **what it would take to
close**:

- **It is not the language.** Rust is C-speed.
- **It is not the value representation (size).** Measured: an 8-byte tagged
  value vs the current 16-byte `enum Value` is only **~1.2×** on the dispatch
  inner loop; a minimal Rust VM using the SAME `enum` representation already
  **matches CRuby** (PoC: enum 0.97× CRuby on fib). So the 5488-site tagged
  rewrite would buy ~1.2% — not worth it as a silver bullet.
- **It is not retrofittable.** Tested and rejected, each empirically: tagged
  value (~1.2%), Rc-removal (a GC-pressure trade, not a win), clone-reduction
  (the clones are necessary result-building), and inlining hot ops to skip the
  per-op `step()` call (**net-zero** — the per-op function-call boundary is not
  the cost).
- **The 5× is STRUCTURAL.** It is the accumulated per-op *work* in a real VM —
  re-fetching the frame from a Vec every op, 16-byte `Value` with a `Drop` impl
  (teardown cost), bounds checks, the 94-arm `step`, the full Ruby semantics —
  woven through the existing design. A minimal VM is fast because it was *built*
  lean (state in registers, Copy values, inlined dispatch, no semantics tax);
  that leanness cannot be retrofitted incrementally.

**PoC validation (the decisive evidence), fib(38), best-of-N:**

| VM | time | vs CRuby |
|----|-----:|---------:|
| Minimal Rust VM, **tagged 8-byte** value | 2.46s | **0.83× (faster)** |
| Minimal Rust VM, **16-byte enum** value | 2.90s | 0.97× (matches) |
| Minimal Rust VM, **tagged + real method dispatch (IC + objects)** | **2.23s** | **0.77× (1.3× faster)** |
| CRuby (fib is an IC-dispatched method) | 2.98s | 1× |
| rubyrs (current interpreter) | 14.2s | 4.8× slower |
| YJIT | 0.51s | (JIT, not an interpreter) |

The last row is the key de-risk: **adding real method dispatch (inline cache +
object header + class check) did NOT erode the lean advantage** — the lean core
with OO dispatch still beats CRuby 1.3×. The "does the architecture survive real
semantics?" risk holds at the most important layer.

## Decision

Build a **new lean VM core from scratch**, designed lean from the first line,
growing one capability layer at a time with a **CRuby benchmark gate at every
layer**. Do NOT attempt to lean the existing 109K-LOC VM in place (proven
non-retrofittable). The existing VM keeps shipping; the lean core grows beside
it as a second execution backend until it reaches parity + feature-completeness.

### The validated lean architecture

- **8-byte tagged value** (`u64`): Fixnum immediate `(n<<1)|1`; aligned heap
  pointer (low bits 0) for objects; nil/true/false as reserved constants;
  flonum or heap-boxed floats. No `Rc`, no `Drop` on the value — heap lifetime
  is the GC's job, so value copy/teardown is free.
- **Register-cached dispatch state**: `proto/ip/base/self` live in loop locals,
  synced to/from the frame only on call/return — NOT re-fetched from a Vec per
  op.
- **Inlined dispatch**: one tight `match` over opcodes in the loop body (no
  per-op function call); unchecked op fetch.
- **Lean frames**: a frame is a base pointer into a contiguous value stack +
  a small control record — no per-frame heap allocation, no 160-byte struct.
- **IC method dispatch**: per-call-site inline cache keyed on the receiver's
  class id (polymorphic ways for megamorphic sites); object header carries the
  class id.
- **GC-managed heap**: tagged pointers into a GC heap; generational
  mark-sweep (the current VM's generational GC design, ADR-era `d81ea567`,
  carries over).

### Coexistence / migration strategy

- Reuse the existing **parser + compiler + preamble + builtin Ruby** where
  possible — the lean core is a new *execution* backend, not a new language.
- A bytecode-translation layer maps the existing `Op`/`Proto` to the lean
  core's opcode set (or the compiler emits lean bytecode directly behind a
  feature gate).
- Ship behind a feature flag; route only fully-supported programs to the lean
  core; fall back to the existing VM otherwise — same opt-in discipline as
  `jit-native`.
- Replace the old VM only once the lean core is at parity AND feature-complete
  AND diff_cruby-clean.

## Build plan — incremental layers, each CRuby-benchmarked

Each layer must stay CRuby-competitive on a representative benchmark before the
next begins (the gate that killed the retrofit approach early, applied
forward).

1. ✅ **Core values + dispatch** (tagged value, stack, frames, arithmetic,
   branches). — fib(38) 2.46s, 0.83× CRuby.
2. ✅ **Method dispatch + objects** (object header, class, method table, IC). —
   fib-as-method 2.23s, 1.3× faster than CRuby.
3. ⬜ **GC** (generational mark-sweep over the object heap) — gate: object-churn
   benchmark stays ≤ CRuby.
4. ⬜ **Strings + the heap value surface** (string as a GC object, no Rc) —
   gate: string-heavy benchmark.
5. ⬜ **Polymorphic / megamorphic dispatch** (multi-way IC, AR's call shape) —
   gate: a megamorphic-call benchmark.
6. ⬜ **Blocks / closures / `yield`** — gate: iterator-heavy benchmark.
7. ⬜ **Exceptions / `ensure` / non-local control** — gate: rescue-heavy.
8. ⬜ **Full builtin surface + the AR CRUD end-to-end** — gate: AR CRUD ≤ CRuby.

## Risks

- **Scope**: this is a multi-quarter-to-multi-year, from-scratch core. The gate
  discipline (benchmark each layer) is what bounds the risk — if any layer can't
  stay competitive, we learn early and stop, not after the whole rewrite.
- **Semantics-vs-leanness tension**: the open question each layer answers is
  whether adding a Ruby feature erodes the lean win (the way it did, cumulatively,
  in the existing VM). Layers 1–2 say no so far.
- **Two VMs to maintain** until parity. Mitigated by sharing parser/compiler/
  preamble and the feature-gate fallback.
- **GC + tagged pointers + unsafe**: the lean core uses `unsafe` (tagged
  pointers, unchecked fetch). Contained behind a typed value API + STRESS_GC
  validation, as the existing heap already is.

## Why this is the right call (not the alternatives)

- **Not the tagged-value retrofit** — proven ~1.2%, the 5× is elsewhere.
- **Not incremental leaning of the old VM** — proven non-retrofittable
  (inline-ops net-zero; the overhead is structural).
- **The JIT (ADR 0030/0032) remains the surpass path for compute** (14.8× YJIT
  on integer methods); the lean core is the surpass path for *framework
  dispatch*, which the JIT cannot reach (megamorphic + huge method surface).

The PoC proved the destination is reachable in Rust. This ADR commits to walking
there by building lean, not by sanding down the existing VM.
