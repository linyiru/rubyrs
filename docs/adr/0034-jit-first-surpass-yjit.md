# ADR 0034 — JIT-first: a full method JIT to surpass CRuby + YJIT

Status: **Accepted (strategy)** — supersedes the standalone framing of ADR 0033

## Context

The goal is sharpened: **surpass CRuby AND YJIT**, not merely match CRuby's
interpreter. That distinction is decisive, and two PoCs settle the strategy.

**An interpreter — however lean — cannot beat YJIT.** ADR 0033's lean VM core
PoC matches/slightly beats CRuby's *interpreter* (fib 0.83×–1.3×), but YJIT is a
JIT and runs the same fib in 0.51s vs the lean interpreter's 2.23s — i.e. the
lean interpreter is still ~4× slower than YJIT. **Only a JIT beats a JIT.**

**A native JIT beats YJIT on BOTH axes that matter — validated:**

| shape | benchmark | rubyrs native JIT | YJIT | CRuby |
|-------|-----------|------------------:|-----:|------:|
| compute | integer method (ADR 0030/0032, shipped) | **14.8× YJIT** | 1× | — |
| compute | recursive fib (ADR 0032, shipped) | **2× YJIT** | 1× | — |
| **framework dispatch** | 900M polymorphic method calls (PoC, this cycle) | **2.20s** | 9.09s | 58.7s |

The framework-dispatch PoC is the new evidence: hand-written native code modelling
a JIT's output — object header + ivar, a **polymorphic inline cache (PIC)**,
method bodies compiled native, the whole loop (incl. `each`/`sum`) lowered — runs
the AR-shaped megamorphic call loop **4.1× faster than YJIT** (26.6× CRuby). The
native JIT's structural edge: **raw values (no VALUE box/unbox), no GC write
barriers, tight PIC + direct calls** — none of which YJIT can have, bound as it
is to CRuby's VALUE/C-API/GC model.

Caveat (honest): the PoC is idealized (an upper bound). A real JIT pays for PIC
fills, deopt guards, and GC interaction. But the **4× headroom absorbs that and
still wins**, and rubyrs's shipped JIT already reaches this tight-native tier on
compute (14.8× YJIT), so the bound is *reachable*, not aspirational.

## Decision

**Prioritise a JIT-first path: grow rubyrs's shipped native Cranelift JIT
(`jit-native`, ADR 0030/0032) into a full method JIT that surpasses YJIT on
framework dispatch as well as compute.** Each capability added beats YJIT on its
shape, so value ships incrementally — unlike the lean interpreter, which would
have to be feature-complete and would *still* lose to YJIT.

ADR 0033's lean VM core is **re-scoped**: not a standalone parity goal, but the
**value substrate + deopt baseline for the JIT** — the JIT compiles to native
code that operates on values, so a lean (tagged, Rc-free) representation makes
the *compiled* code faster, and a lean interpreter makes the *deopt/cold* paths
faster. Lean values serve the JIT; the JIT is the surpass weapon.

## Architecture

A **full method JIT**: lazily compile hot Ruby methods to native (Cranelift),
guarded by a **polymorphic inline cache** with **deoptimisation** to the
interpreter on any failed speculation, operating on a **lean value substrate**.
This is the YJIT design, with rubyrs's structural advantage (whole-stack control
→ raw values, no C-API tax).

Builds on shipped infrastructure (ADR 0030/0032, `crates/rubyrs/src/jit_native.rs`):
`NativeProto` + Cranelift lowering, `guard_class` monomorphic speculation, deopt
on overflow/type, cross-method native calls (compilation scope), value primitives
(ivar/array/hash/string), `jit_should_route` selective routing.

## Roadmap — incremental, each layer beats YJIT on its shape, each gated

1. ✅ **Compute kernels** (integer + value methods, cross-method calls, array
   building) — shipped, 14.8×/6.5× YJIT/CRuby.
2. ⬜ **Polymorphic inline cache (PIC)** — extend the monomorphic `guard_class`
   to N-way speculation + deopt. The PoC's core; gate: a polymorphic-dispatch
   benchmark beats YJIT (PoC says 4× headroom exists).
3. ⬜ **Lazy hot-method compilation** — a call-count threshold triggers
   compilation (YJIT-style), so framework methods enter the JIT when hot. Gate:
   a method-call-heavy benchmark with warmup beats YJIT end-to-end.
4. ⬜ **Method-body coverage** — lower the op/semantic surface real framework
   methods need (conditionals, kwargs, blocks-as-args, more value ops, alloc).
   Gate: progressively larger real methods compile + stay > YJIT.
5. ⬜ **Inlining** — inline small monomorphic callees (getters/predicates) into
   the caller's native code. Gate: getter-chain benchmark.
6. ⬜ **Lean value substrate (ADR 0033)** — tagged values at the JIT boundary;
   raw values through compiled call trees. Gate: the compiled code's value ops
   match the PoC's raw-i64 tier.
7. ⬜ **Deopt completeness** — robust deopt on every speculation (class redefine,
   method redefine, type, overflow, refinement). Gate: correctness under
   adversarial redefinition (diff_cruby + dedicated deopt tests).
8. ⬜ **AR CRUD end-to-end** — enough framework coverage that real AR CRUD runs
   JIT-compiled and beats YJIT. Gate: AR CRUD < YJIT's 246ms.

## Layer 3 exploration (2026-06-28) — the bottleneck is *firing coverage*, not codegen

Before building layer 3 we measured where the JIT actually stands on realistic
dispatch shapes (`RUBYRS_JIT_NATIVE=1`, release, best-of-3 wall ms):

| shape | rubyrs interp | **rubyrs jitN** | YJIT | jitN vs YJIT |
|-------|--------------:|----------------:|-----:|--------------|
| instance-method `fib(35)` (fires) | 3.22 | **0.09** | 0.21 | **2.3× faster** |
| driver+leaf both instance methods, leaf pre-warmed (fires) | 2.51 | **0.04** | 0.48 | **12× faster** |
| top-level `fib(35)` | 3.09 | 3.09 | 0.19 | 0 win |
| `while` loop + top-level leaf call | 2.76 | 2.74 | 0.53 | 0 win |
| `while` loop + method on an Object element | 2.34 | 2.40 | 0.47 | 0 win |
| block iterator `rows.sum { _1.amount }` (real AR shape) | 6.94 | 7.00 | 0.51 | 0 win |
| megamorphic `shapes[i%4].tag` | 6.51 | 6.72 | 1.50 | 0 win |

**The decisive finding: when the JIT fires it already beats YJIT 2–12×. The
problem is it never fires on realistic *driver loops*.** The compiled code is not
the bottleneck — eligibility is. Five gates block firing, in leverage order:

- **B1** — only an *instance method on a `Value::Object` receiver* routes to
  native; a top-level `<main>` loop or a `no_recv` call does not. (top-level fib
  = 0 win, instance fib = 0.09s, same body.)
- **B2** — `Mod`/`Div` are rejected by the op-gate (`jit_native.rs`), so any loop
  with `i % n` cycling declines. (the megamorphic shape, and much real code.)
- **B3** — a callee must already be compiled when its caller compiles, else the
  cross-call op makes the *caller* decline. (callee-before-caller ordering.)
- **B4** — a method call on an Object *inside* the driver (`rows[j].amount`) can't
  be lowered → needs the slow-path re-entrant call helper.
- **B5** — blocks (`.sum {}`, `.times {}`, `.map`) can't be compiled at all —
  the dominant real AR/AM shape, and entirely untouched.

### Re-prioritised layer-3 plan (supersedes the "re-entry → PIC → value" order)

The original sketch was *re-entrant helper → PIC codegen → general value
compiler*. The data re-orders it:

1. **Gate widening (B1+B2+B3)** — zero new codegen; converts already-validated
   12×-YJIT compiled code from the lab (driver-compiled shape) into wins on real
   top-level driver loops. Lowest risk, shipped first. **← this PR.**
2. **Re-entry helper (B4) bundled WITH inline PIC codegen** — measured bound: a
   fully-interpreted object-method loop is ~125 ns/call vs ~2 ns/call for the
   native driver. A re-entry helper that re-enters `do_call` per call costs ≈ the
   interpreter's dispatch, so it **cannot win alone** — object/polymorphic
   dispatch needs a native driver + an *inline* PIC direct call, with the
   re-entry helper as the deopt/megamorphic fallback. (Contradicts "re-entry
   first, PIC later".)
3. **Blocks + general value compiler (B5)** — the real AR/AM block-iterator shape;
   largest untouched block, hardest (must compile `yield`).

The roadmap layers above still hold; this section records *how to sequence them*
given the measured evidence that codegen quality is already past YJIT.

### Shipped: B1+B2+B3 (gate widening) — top-level method drivers now fire

The first PR landed B1+B2+B3 (no new codegen). Measured (`RUBYRS_JIT_NATIVE=1`,
release, best wall ms):

| shape | before | after | YJIT | result |
|-------|-------:|------:|-----:|--------|
| top-level `fib(35)` | 0 win | **0.08** | 0.20 | **2.5× YJIT** |
| top-level `run` + top-level `inc` (`s11`) | 0 win | **0.03** | 0.43 | **14× YJIT** |
| top-level `run` + `sq(i % 1000)` (`s5`) | 0 win | **0.07** | 0.51 | **7× YJIT** |
| instance driver, no warmup (`s9`) | needed warmup | **0.03** | 0.43 | **14× YJIT** |

- **B2** — `Mod`/`Div` lowered with branchless Ruby floored semantics (`select`),
  deopting on `b == 0` and `i64::MIN / -1`. Negative floored div/mod match CRuby.
- **B3** — callees compiled on demand (recursive, with a `visited` cycle guard);
  callee-before-caller warmup no longer required; mutual recursion declines
  safely instead of looping.
- **B1** — native dispatch added inside `try_invoke_fixed_method_from_stack`
  using the *already-resolved* method (resolution semantics unchanged: top-level
  def vs `Kernel`, redefinition, `load` shadowing all preserved). Callee
  resolution falls back from the receiver class to the `toplevel_methods` table,
  so a top-level driver calling another top-level method compiles its whole tree.
- **Validation** — `diff_cruby --features jit-native` adds **zero** new failures
  (the only JIT-on deltas remain the two pre-existing quirks
  `attr_reader_getter_fastpath` / `comparison_failed_message`).

Still 0-win (expected, later layers): bare-`<main>` while-loop drivers (need the
`<main>`/0-arg proto itself compiled), object-method calls inside a driver (B4),
and block iterators (B5).

### B4 — soundness pivot + step 1 (self attribute-reader calls)

Building B4 surfaced a soundness result that re-shapes its design: **an
after-the-fact "deopt = re-run the whole method in the interpreter" is UNSOUND
for a re-entrant call whose callee has side effects** (the redo repeats them).
The existing JIT's deopt is sound only because every compiled op is pure (local
rw, int arith, reads, method-local arrays, and calls to *other compiled-pure*
methods). So B4 keeps that invariant rather than adding an arbitrary re-entry:

- **Compile only pure drivers** (the op-gate already admits no side-effecting op)
  — so whole-method redo stays behaviour-preserving.
- **Inline PIC, deopt-to-interpreter as the slow path** (not a re-entry helper):
  on a class-guard miss the whole pure driver re-runs interpreted. A re-entry
  helper that re-enters `do_call` per call is deferred — it can't beat the
  interpreter's own dispatch anyway (the masked-leaf measurement), and it
  re-introduces the side-effect-redo hazard. The interpreter handles the
  megamorphic tail.

**Step 1 (shipped): a 0-arg bare call to a simple int attribute reader
(`amount` → `@amount`) is lowered to an INLINE ivar read** on the receiver — no
frame, no dispatch. Resolved via the proto's precomputed `getter_ivar`, guarded
to the resolving class. A non-Int ivar deopts (sound: pure read). Measured
(`s += amount` aggregation driver, 20M calls): **0.06s vs YJIT 0.44s — 7×**.
diff_cruby unchanged. Next: array-element receivers (`rows[j].amount`) with a
real inline class-guard PIC — the megamorphic AR shape (s2).

## Risks

- **YJIT-class scope.** A full method JIT with PIC + deopt + broad coverage is a
  multi-quarter-plus effort. Bounded by the gate discipline: every layer must
  beat YJIT on its shape or we stop and reassess — and crucially, **every layer
  ships a real win** (a faster compiled shape), so there is no "complete or
  worthless" cliff.
- **Deopt correctness** is the hard safety property (speculation must be
  perfectly reversible). Mitigated: deopt-can-only-change-speed already holds for
  the shipped JIT (overflow/type), extended per-speculation with tests.
- **Megamorphic tail** (sites with > N classes) degrade to the interpreter —
  acceptable (YJIT does too); the lean interpreter (0033) keeps that floor fast.
- **`unsafe` + native codegen** — contained behind the existing `jit-native`
  feature gate + STRESS/diff_cruby validation; opt-in/trusted, as today.

## Relationship to other ADRs

- **0030 native JIT / 0032 surpass** — the shipped foundation this extends.
- **0033 lean VM core** — re-scoped as the JIT's value substrate + deopt
  baseline (not a standalone parity goal).
- **The JIT is the surpass path for BOTH compute and framework dispatch**; the
  lean interpreter only ever matched CRuby's interpreter and lost to YJIT — so
  for the "surpass CRuby+YJIT" goal, JIT-first is the only validated route.

The two PoCs prove the destination (beat YJIT on compute AND dispatch) is
reachable with native code + a PIC. This ADR commits to getting there by growing
the JIT, layer by layer, each layer a shipped win over YJIT.
