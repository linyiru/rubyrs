# ADR 0034 — JIT-first: a full method JIT to surpass CRuby + YJIT

## Status

**Accepted (strategy)** (2026-06) — supersedes the standalone framing of
[ADR 0033](0033-lean-vm-core.md), and reverses the "no JIT" decision of
[ADR 0002](0002-bytecode-vm-not-jit.md).

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
diff_cruby unchanged.

**Step 2 (shipped): array-element attribute reads with a real inline PIC.** The
AR aggregation `t += @rows[j].amount` (a collection ivar, variable index, int
attribute) is fused into one primitive `jit_arr_elem_attr_int` doing array-get +
element-class inline cache + attribute read. The cache is a per-site
`Box<Cell<(class_ptr, ivar)>>` owned by the `NativeProto`, its address baked into
the code as a constant — monomorphic: a hit reads the cached ivar, an empty cache
fills it (resolve `getter`→`getter_ivar` on the element's class), and a class
miss / non-Int / OOB / non-Object case deopts the whole pure driver to the
interpreter (which handles the megamorphic tail). Measured (`@rows[j].amount`,
1000×20k elements): **0.28s vs YJIT 0.51s — 1.8×**. Correctness verified against
CRuby: megamorphic alternating classes, a non-Int (Float) attribute, and a
non-Object array element all deopt to the exact interpreter result; STRESS_GC
clean; diff_cruby unchanged.

Next within B4: a polymorphic (N-way) cache instead of monomorphic-or-deopt, so a
genuinely megamorphic site stays native; then B5 (block iterators).

### B5 — native blocks called from the Rust iterator drivers

Investigation reshaped B5: `sum`/`map`/`each`/`times` are **Rust iterator
drivers** (a Rust loop calling `step_block*` per element), not Ruby bytecode with
`Op::Yield`. So the win isn't compiling `yield` — it's making the Rust driver call
a **native-compiled block** instead of re-entering the interpreter per element.

**Step 1 (shipped): a 1-param pure-int block runs native.** `step_block1` gains a
fast path: if the block is a 1-param (no rest/kw/block-param) pure int function of
its param, it's compiled to a `NativeProto` (`jit_native_block` cache) and called
directly — no frame, no `dispatch_until`. The block compile reuses the method
codegen with two changes: the arg binds to the block's `param_start` (not local
0), and any read of a captured OUTER slot declines (so only a pure function of the
param + the block's own temporaries qualifies — a closure over outer state stays
interpreted). Soundness is automatic: a native block is pure (the op-gate admits
no side effect), and a deopt commits nothing, so the interpreter re-runs cleanly.

A `Return` kind-gate was added: a Bool/Nil result now declines (a block
`{ |x| x > 5 }` returns true/false, not 1/0) — which also fixed the pre-existing
`comparison_failed_message` jit quirk.

Measured (`(0...1000).map { |x| x*x }.sum`, 20k iters): **0.42s vs YJIT 0.73s —
1.7×**; `arr.sum { |x| x*x }`: **0.34s vs YJIT 0.45s — 1.3×**. Correctness verified
vs CRuby: a capturing block, a block with a method call, and a Float-returning
block all fall back to the interpreter; a block reading a self ivar runs native;
STRESS_GC clean; diff_cruby drops to 10 failures (one fewer — the fixed quirk).

**Step 2 (Object-param blocks) — investigated, NOT shipped.** `rows.sum { |r|
r.amount }` was made to compile (a `Kind::Object` param + an inline-PIC attribute
read on it). It is correct on every edge (megamorphic, Float, non-Object → all
match CRuby), but it **does not beat — and slightly regresses — the
interpreter** (1.47s vs 1.38s): the interpreter's frame-free getter fast path is
already cheap, so the per-element native-block bookkeeping (block-handle read,
cache lookup) costs more than it saves. Per the gate discipline ("beat YJIT or
stop"), it was reverted. The native-block win stays where the block body is real
work (Int arithmetic), not a single getter.

The chase surfaced a separate, ungated win, shipped independently: a native Rust
`Array#sum`-with-block driver replacing the preamble's
`each { |*x| memo += yield(*x) }` (whose accumulator block — capture + splat +
re-yield — can never go native). **`rows.sum { |r| r.amount }`: 6.97s → 1.38s,
~5×**, on every build.

### Layer 3 turning point: whole-loop native compilation (beats YJIT 6×)

The per-shape data forced a structural conclusion. Native blocks called *per
element through a Rust iterator driver* (`step_block1`) win only when the block
body is substantial arithmetic (`sum { x*x }`: 1.3× YJIT) — and **lose** on
trivial bodies (a single getter, `t += x`, a predicate). Cause: the per-element
Rust dispatch + block-handle lookup costs more than the body saves, while YJIT
inlines the entire loop into one tight native function. Measured across capture
accumulators, 2-param inject, and `each_with_index`, all trivial-body shapes sat
*at or below* the interpreter under jit-native — the cheap per-block increments
were exhausted.

The lever is **compiling the whole loop**, not the block alone. `Array#sum
{ int-block }` now compiles to a single native function (`compile_native_sum_loop`):
read the length once, then a native loop that reads each element
(`jit_array_elem_int`, deopt on non-Int), calls the already-compiled native block
(a tight native→native call, no interpreter re-entry), and accumulates with an
overflow-checked add — one shared `deopt` exit for non-Int element / i64
overflow, falling back to the generic loop (sound: a native block is pure, so a
part-way deopt doubles nothing on redo).

| shape | interp | **jitN** | YJIT | vs YJIT |
|-------|-------:|---------:|-----:|--------:|
| `arr.sum { x*x }`            | 1.12s | **0.07s** | 0.44s | **6.3×** |
| `arr.sum { x*x + x*2 + 1 }`  | 1.88s | **0.08s** | 0.46s | **5.75×** |
| `arr.map { x*x + 1 }`        | 1.34s | **0.17s** | 0.55s | **3.2×** |
| `arr.count { x > 500 }`      | 1.17s | **0.07s** | 0.43s | **6.1×** |
| `arr.select { x > 700 }`     | 1.22s | **0.10s** | 0.48s | **4.8×** |

| `arr.any? + all?`            | 2.33s | **0.14s** | 0.78s | **5.6×** |
| `arr.find { x > 700 }`       | 0.86s | **0.05s** | 0.43s | **8.6×** |
| `arr.inject(0){ s+x*x }`     | 1.45s | **0.07s** | 0.60s | **8.6×** |

`select` / `reject` / `filter` reuse predicate-mode blocks with a third loop
shape (the `Filter` kind): per element, run the predicate and on the kept polarity
push the *element* into a result the caller reserves to the input length (so the
native push never reallocs — no GC, no move). Order is preserved; a non-comparison
predicate (`x.odd?`) or `break` declines to the generic walk.

`inject` / `reduce { |acc, x| }` add a 2-param block ABI: `compile` gained a
`two_param` flag that adds a second i64 arg and binds it to `param_slot + 1` (both
slots are params, not captures). The accumulator threads through the block's
return value — no closure state — so `inject(init) { }` over an all-Int array
folds entirely native (`compile_native_inject_loop`). The no-init form (`acc =
arr[0]`) and a non-Int init/result stay generic. This unlocks the path to
`each_with_index` and other 2-param drivers.

`find` / `detect` add a fourth loop shape (`Find`): a predicate loop that pushes
the first match into a capacity-1 result and **early-exits**, so it is the fastest
driver of all (8.6× — it stops at the match rather than walking the whole array).
The caller reads `arg2[0]` or treats an empty `arg2` as "not found" (→ nil).

`any?` / `all?` / `none?` / `one?` need no new codegen: a PURE predicate has no
side effects, so the early-exit forms are observationally equal to a full count of
the matches. They run the native count loop and compare (`any? = c>0`,
`all? = c==len`, `none? = c==0`, `one? = c==1`); empty arrays fall out correctly
(c=0). Predicate mode is tightened to lower **only** a `Bool` result to 0/1 — a
non-Bool truthy result (`count { |x| x }`, every Int truthy) declines to the
generic walk rather than summing raw values. The three loop compilers were then
unified into one `compile_native_loop(block_addr, kind)` (`Sum` / `Map` /
`Filter`), so a new driver is a match arm, not a 150-line copy.

`count` reuses the sum loop unchanged: a count IS a sum of the predicate's
truthiness. The new piece is **predicate-mode block compilation** — a final `Bool`
Return (an `icmp` result) materialises as i64 0/1 instead of declining — which
unlocks the whole predicate family (count / select / reject / find / any? / all?
/ none?). A non-comparison predicate (`x.even?`, a method call) still declines to
the generic loop. Alloc-free, so it matches `sum`'s 6× margin.

`map` extends the template: the caller pre-sizes the result to the input length
(so the native store never reallocs — no GC, no element move mid-loop), reads
`self_val` only *after* that last allocation (sidestepping a rooting hazard), and
the loop fills it via `jit_array_set_int`. A non-Int element / non-Int block
result / overflow deopts and the partial result is dropped for the generic loop.
The margin is smaller than `sum` (a per-element store + the up-front alloc) but
still clears YJIT 3.2×.

This is the first **whole-loop** layer and the empirical turning point: eliminating
per-element interpreter overhead clears YJIT by ~6×, where per-element native
blocks could not. STRESS_GC clean (the native loop is alloc-free), diff_cruby
unchanged (10 jit-native / 9 default), correctness verified vs CRuby (Float /
Bignum seed / `break` / empty / Range / Object-block all match).

### Shipped drivers (this layer)

`sum` (6.3×), `map` (3.2×), `count` (6.1×), `select`/`reject`/`filter` (4.8×),
`any?`/`all?`/`none?`/`one?` (5.6×), `find`/`detect` (8.6×), `inject`/`reduce`
2-param (8.6×), `min_by`/`max_by` (8.6×) — all over an all-Int array with a
comparison/arithmetic block; non-Int elements or results, method-call keys/
predicates (e.g. `.abs`, `.even?`), `break`, and the no-init `inject` decline to
the generic walk. Four loop shapes (`Sum` / `Map` / `Filter` / `Find`) under one
`compile_native_loop`, plus the 2-param `compile_native_inject_loop` and the
best-tracking `compile_native_minmax_loop` (strict `<` / `>`, so ties keep the
first element — CRuby-stable; the empty array is left to the generic walk, which
yields nil).

### Layer 3b: inline pure unary Int primitives

A recurring ceiling surfaced above: **method-call keys/predicates** (`.abs`,
`.even?`, `.positive?`) declined because the block op-gate modelled only arithmetic
+ comparison. The gate now lowers six pure unary Int primitives **inline** (no
call, `emit_int_unary`): `abs` → `Int` (an `i64::MIN` negation overflows → deopt);
`even?`/`odd?`/`zero?`/`positive?`/`negative?` → `Bool` (an `icmp`, usable as a
comparison condition or a predicate-block result). Crucially these handle the
**fused `LoadLocalCall`** op — `x.abs` compiles to `LoadLocalCall(slot, abs, …)`,
not `LoadLocal + Call`, so without that arm the blocks declined at the gate. Result
over an all-Int array:

    count { |x| x.even? }   jitN 0.07s vs yjit 0.44s   6.3×
    sum   { |x| x.abs }     jitN 0.07s vs yjit 0.47s   6.7×
    min_by { |x| x.abs }    jitN 0.07s vs yjit 0.69s   9.9×

**Two soundness fixes the new coverage exposed** (both latent — the blocks/methods
previously declined for other reasons):

1. *Block branch-merge.* `x*2 if x.even?` merges an `Int` (the then-value) with a
   `Nil` (the missing-else) at one block. `block_args` recorded only the first
   branch's kinds, so the nil branch silently lowered to i64 `0` — `filter_map`
   then kept it (`[0,4,0,8]` for `[4,8]`). `block_args` now returns `None` on a
   kind-shape mismatch, declining the proto so the interpreter runs it.
2. *Value-JIT arity.* `compile_value` matched the plain getter shape
   `[LoadIvar, Return]` regardless of arity, so a 0-param getter served a
   *wrong-arity* call (`obj.x(1)` / `send(:x, 1)`) from the 1-arg dispatch hook,
   swallowing the `ArgumentError`. It now applies `compile`'s shape gate (exactly
   one required positional). This closed the lone jit-native-specific `diff_cruby`
   failure — the JIT build is now at exact parity with the interpreter (both green,
   9 known missing-feature TODOs quarantined as `#[ignore]`).

The mutable-capture family (next) is the remaining ceiling.

### Deferred (designed): the `each` accumulator

`total = 0; arr.each { |x| total += x }` is the one common reader that needs
**mutable captured state** — the block writes an outer variable. That breaks the
deopt-redo soundness every other driver relies on: a part-way deopt has already
committed writes to the real (shared) method scope, so re-running from the start
would double them. Two sound designs exist:

1. **Continue-from-deopt-index.** The native loop writes captures directly and
   returns the index it reached; on deopt the caller runs the generic `each` over
   `arr[reached..]` (the shared scope already holds `0..reached`'s contributions).
   Sound only with an *element-atomicity* gate: no deopt-able op may follow the
   first capture write within an element, else continuing re-does a committed
   write. Requires capture get/set primitives, a `vm`-stashed captured pointer,
   capture lowering in codegen, and the atomicity pre-pass.
2. **Inject-with-captured-acc.** Recognise the single-accumulator pattern
   (`read capture S → pure int work → write capture S`, S the only capture),
   read `S` once into a register, run the existing inject loop, write `S` back
   once at the end. A mid-loop deopt has touched nothing → redo-from-scratch is
   sound, and it reuses `compile_native_inject_loop`. Needs reliable bytecode
   pattern recognition (and there is no bytecode-dump tool yet to verify it).

### Layer 3c: each-accumulator — SHIPPED (approach 2)

`arr.each { |x| total += x }` now compiles, **6.1× YJIT** (jitN 0.07s vs 0.43s).
Approach 2 as designed: an each-accumulator IS `inject` with the accumulator bound
to a CAPTURED slot. `compile`'s block spec gained an `AccKind` enum
(`None`/`Inject`/`EachAcc{acc_slot}`) saying where the accumulator vs element bind
on the C ABI; the new acc is the captured slot's variable *after* the body (not the
block's discarded return), so `total += x; total*2` can't corrupt it.
`try_native_each_acc_loop` finds the single written captured slot, seeds from it,
runs `compile_native_inject_loop`, writes back. **Two soundness gates** (both now
reusable for the rest of the family):

1. *Deopt = write-back-only-on-success.* The slot is committed once, on full native
   completion. Any deopt (non-Int element/result, overflow) commits nothing → the
   seed is intact → the caller redoes the whole `each` generically. Verified:
   `3×2^62` overflows mid-loop → correct bignum, no double-count.
2. *Share-direct-only.* Fires only when the block takes the share-direct frame path
   (`captured_is_method_scope && !creates_block && !reentrant` — exactly
   `block_frame_locals`' condition), so a direct `captured[slot]` write is the live
   scope. The COPY path (nested `[[1,2]].each { |p| p.each { |n| total += n } }`,
   where `captured` is an enclosing block's per-invocation copy with a write-back
   chain) declines to the generic walk. The now-green baseline caught the missing
   gate as a real regression (`block_locals_share`) on the first run.

**each_with_object — SHIPPED**, 4.7× YJIT (jitN 0.03s vs 0.14s). `arr.each_with_object([])
{ |x, memo| memo << f(x) }` over an all-Int array building an Array. The `memo` param
binds to a fresh SCRATCH array (kind `ArrayObjId`) the block pushes into via the
existing `<<` codegen; `compile_native_eachobj_loop` allocates the scratch, threads
it through every block call, returns its ObjId; the driver appends scratch onto the
real `memo` on success. Gate #1 again: a deopt discards the scratch, the real memo is
untouched, redo generic — verified `big*4` overflow mid-loop → correct bignum, no
stray scratch element. (`memo` is a real param here, so no capture-path gate #2 is
needed; non-`<<` use of `memo` declines in `compile`.) The block spec now models the
accumulator/element binding as explicit per-`AccKind` `arg2_slot`/`arg3_slot` — which
also fixed a latent `Inject` binding regression the each-accumulator refactor had
introduced (inject was silently declining to generic; back to 9.9× YJIT).

**group_by — SHIPPED**, 6.3× YJIT (jitN 0.03s vs 0.19s). `arr.group_by { |x| key }`
over an all-Int array with an Int key. `compile_native_groupby_loop` allocates a
fresh result Hash (`jit_hash_new`), runs the value-mode key block per element
(shared with the sum/map/min_by block cache), and buckets each element under its key
via `jit_group_push` (find-or-create the bucket Array, first-appearance order =
CRuby). Both primitives are GC-free, so the in-flight Hash + buckets stay live across
the loop. Gate #1: the Hash is fresh / never user-visible until success, so a deopt
(non-Int element or key) discards it — no partial Hash escapes; no capture, so gate
#2 is moot.

**tally — SKIPPED** (evaluated): it takes no block, so the generic `Array#tally` is
already a native Rust loop — there is no per-element interpreter dispatch to remove.
A reminder that the JIT win is *block-dispatch elimination*; a blockless reducer
that's already native Rust gains nothing.

**each_with_index — SHIPPED** (the last capture member), 9.4× YJIT (jitN 0.07s vs
0.66s). `total = …; arr.each_with_index { |x, i| total += f(x, i) }`. Like the
each-accumulator, but the block has a 2nd param (the index), passed as a 3rd i64 block
arg — `compile`'s block spec now carries explicit `arg2/arg3/arg4` slot binding +
`AccKind::EachWithIndex`; `compile_native_eachidx_loop` threads the accumulator and
passes `i`. Same two gates (write-back-on-success + share-direct-only).

**The capture family is complete**: each-accumulator (6.1×), each_with_object (4.7×),
group_by (6.3×), each_with_index (9.4×) — all over an all-Int array, all on the two
reusable soundness gates. `tally` was the one evaluated-and-skipped member (no block);
its real cost (an O(n²) generic build) was fixed in the interpreter instead (now 2–4×
YJIT).

### Layer 3d: lifting the all-Int restriction — Float `sum` — SHIPPED

`floats.sum { |x| <pure f64 expr> }` over an all-Float array now compiles to one
native loop — **6.75× YJIT** (jitN 0.08s vs 0.54s; before, **14× SLOWER** than YJIT:
the Int `sum` driver declined on the Float elements, so the block ran interpreted
per element). The first Float-element driver.

The lever that kept this cheap: Float values flow through the **existing uniform
i64 ABI by bit-pattern**, so no driver/ABI was duplicated and the Int paths are
provably untouched. A Float local's i64 var holds the f64 *bits*; `LoadLocal`
bitcasts I64→F64, `StoreLocal`/`Return` bitcasts F64→I64. New: `Kind::Float`,
`emit_binop_float` (fadd/fsub/fmul/fdiv — IEEE, no overflow; comparisons + `%`
decline), `LoadConstFloat`, a `float_elem` compile flag binding the 1-param
element as Float, `jit_array_elem_float` (deopt on non-Float), and
`compile_native_floatsum_loop` (F64 accumulator). The driver **declines on an empty
array** so CRuby's bare-init type holds (`[].sum{}` → Integer `0`, not `0.0`).

Re-benchmarked the shared codegen after the change: Int sum/inject/each-acc all
still 0.07s (no regression). Parity (transform/identity/int-seed/mixed-deopt/
empty/+Inf/−0.0/÷0) + STRESS_GC clean + diff_cruby GREEN both builds. cranelift
0.133 note: `bitcast` takes `MemFlagsData::new()` (0.133 split `MemFlags` into an
interned handle + the `MemFlagsData` payload).

cranelift 0.133 note: `bitcast` takes `MemFlagsData::new()` (0.133 split `MemFlags`
into an interned handle + the `MemFlagsData` payload).

**Float drivers extended — `map`, each-accumulator, `min_by`/`max_by` — SHIPPED.**
All three reuse the bit-pattern-through-i64 mechanism:
- **map** (Float→Float): 3.10× YJIT. `jit_array_set_float` + `compile_native_floatmap_loop`.
- **each-accumulator** (`total += f(x)`, Float total): 7.37× YJIT. The inject loop is
  parameterised on the element reader (`compile_native_floatinject_loop`) — the
  accumulator threads as opaque i64 bits, so one loop serves Int and Float; `compile`'s
  `float_elem` marks both the captured acc and the element Float for `EachAcc`.
- **min_by/max_by** (Float key): 8.75× YJIT. `compile_native_minmax_loop` parameterised
  on the element reader; keys compare via ordered `fcmp` on the bitcast bits. A **NaN
  key deopts** (`fcmp Unordered`) so CRuby's "comparison failed" raise reproduces on
  the generic path instead of a silent wrong answer.

**Int↔Float coercion — SHIPPED.** A numeric binop with mixed Int/Float operands
coerces the Int to f64 (`fcvt_from_sint`), matching Ruby promotion — so the COMMON
form `floats.sum { |x| x * 2 }` (bare Int literal) goes native: 6.38× YJIT (was
interpreted — mixed kinds used to decline). `emit_numeric_binop` centralises the rule.

**Soundness fix (latent, in the shipped floatsum/floatmap):** the value-mode `Return`
now enforces result-kind = loop-kind via `float_elem`. A Float driver whose block
returns a non-Float (`floats.sum { |x| 5 }`) was reinterpreting Int bits as f64 →
garbage; it now declines to generic. Coercion made the symmetric Int-driver case
reachable too; both directions are now guarded.

**Float predicates — `select`/`reject`/`count`/`find` — SHIPPED.** count 7.0×,
select 4.46×, find 11.2× YJIT (any?/all?/none?/one? ride the count path). The block
is a float comparison (`x > 2.0`): `emit_binop_float` lowers comparisons to ORDERED
`fcmp` (+ `NotEqual`), which match Ruby's float NaN semantics WITHOUT a special case
(`NaN < x`/`NaN == x` false, `NaN != x` true). `compile_native_loop` is parameterised
on the element reader + push fn; select/reject/find push the float element via
`jit_array_push_float`. Mixed coercion works in predicates too (`x * 2 > 3`).

**`map { b }.sum` fusion — SHIPPED.** A compile-time, in-place peephole: `recv.map
{ b }.sum` (Enumerable) rewrites `CallBlock(map, 0)` → `CallBlock(sum, 0)` and drops
the sum call — one pass, no intermediate Array. Emit-time + in-place ⇒ no index/jump
shifts. `floats.map { x*2 }.sum`: 0.3s → 0.08s (8.0× YJIT).

**Blockless `Array#sum` Float fix (interpreter, both builds).** Surfaced while
profiling the above: `floats.sum` (no block) only fast-pathed Int+Int and fell to the
interpreted `Enumerable#sum` preamble on the first Float — 14.7s → 0.08s (**184×**).
Native f64 accumulation (Int→Float promotion). Like the tally fix, an algorithmic win
independent of the JIT.

**Method-body Float returns — SHIPPED.** A method producing a Float (`def scale(n);
n*1.5; end`) compiles native and boxes `Value::Float` (new `returns_float` flag);
9.36× YJIT. A method mixing Float and non-Float returns declines (ambiguous box kind).
Method ARGS stay Int-only (the dispatch guard) — Float params are the next step.

**Float method PARAMS via cross-call inlining — SHIPPED.** A 1-arg method called with
a Float arg goes native; the win is INLINING — a compiled caller inlines a float-param
callee (`run`→`poly(k*0.5)`) as a native cross-call (`compile()` gains a `float_callees`
map; a Float-arg `CallNoRecv` calls the `cf{cid}` fparam address, f64 bits through the
i64 ABI). 10.92× YJIT (interp 6.06s → 0.12s). A dispatch-only fallback covers
non-inlinable sites (correct, ~2.2× over interp, below YJIT — no dispatch-based call
beats YJIT, only inlining does).

**Blockless `reduce(:+)`/`inject(:+)` Float fold — SHIPPED (both builds, ~150×).** The
reduce analog of the blockless-sum fix: only Int×Int fast-pathed, the first Float fell
to the interpreted `Enumerable#inject` (16.5s → 0.11s). The real bug the "map→reduce(:+)
fusion" ask surfaced (profile the WHOLE chain).

**Int-element / Float-accumulator `sum` — SHIPPED (7.0× YJIT, ~100× swing).** `ints.sum
{ |x| x*1.5 }` (Int collection → Float) had no driver (Int loop declines on the Float
result; Float loop deopts on the Int element) → 14.1× slower. Fix decouples element-kind
from accumulator-kind: a new `float_acc` param on `compile()` governs the value-result
(existing callers pass `float_acc == float_elem`, behaviour-identical), and
`compile_native_floatsum_loop_inner(_, int_elem)` swaps the element reader to
`jit_array_elem_int`. The broad Int→Float number-crunch prize.

**Fusion asks resolved without new fusion:** `arr.map{}.reduce(:+)` already = 3.46× YJIT
(native float map + the blockless reduce fix); `select{}.map{}.sum` = 5.19× (the existing
map.sum peephole fuses `.map{}.sum`→`.sum{}`, select native). Fusing map.reduce(:+)→sum
is unsound (string concat, empty→nil vs 0, −0.0 seed all differ from sum).

**Int-element / Float-block family — SHIPPED (all beat YJIT).** Every Int-collection
crunched into a Float now goes native, via one recipe: decouple element-kind
(`float_elem`, picks the int vs float array reader) from accumulator/result-kind
(`float_acc`, governs the value-Return + the inject/each-acc accumulator slot); existing
callers pass `float_acc == float_elem` (behaviour-identical). Drivers: sum 7.0×, map
2.74×, each-acc 6.86×, inject 11.33× (int-elem) / 8.10× (the previously-missing Float
2-param inject), each_with_index 13.19×, min_by/max_by 10.79× (a second decouple —
`float_key` separates the loop's key comparison from its element reader). Float `#abs`
(fabs) is now modelled in the block JIT (was Int-only), unblocking the canonical
`min_by { (x-t).abs }` (12.58×) cross-cutting across all Float drivers. Predicates over
Int elements with a Float comparison (count/select/find) already beat YJIT, no work
needed.

Float sign predicates `positive?`/`negative?`/`zero?` are now modelled too (ordered fcmp
against 0.0 → Bool), so `floats.count { x.positive? }` etc. beat YJIT (6.06×).

Float→Int conversions now ship too — `floor`/`ceil`/`to_i`/`truncate`/`round` (the reverse
asymmetry: Float element, Int result). They lower to `fcvt_to_sint` after the op's integral
rounding (`round` is Ruby half-away-from-zero = `fcvt(x + copysign(0.5,x))`, NOT cranelift
`nearest`=half-even), each with a branchless range-guard deopt (out-of-i64-range / NaN / Inf
→ generic, where Ruby gives a bignum / raises). The key reuse: the Float-reader/Int-acc sum
loop already existed (`compile_native_floatloop` + `LoopKind::Sum`, the count driver's), and
the generic `LoopKind::Map` already stores Ints, so the only new piece was a Float-elem/
Int-result block. `floats.sum { x.floor }` 6.21× / `{ x.to_i }` 5.74× / `{ x.round }` 6.53×;
the map fires + is parity-correct (alloc-bound, so it tracks the 3.10× float→float map).

The capture/Hash family is Float-extended too. group_by now covers all three key/element
combos: `floats.group_by { x.floor }` (Float element / Int key) and `floats.group_by { x*2.0 }`
(Float element / Float key — `jit_group_push_floatkey` matches buckets by replicating
`Value::ruby_eql`'s Float arm, NaN-by-bits / else `==`, so −0.0 and 0.0 merge exactly as CRuby
and the result is identical to the interpreter's). `floats.each_with_object(memo) { |x, m| m <<
f(x) }` runs natively (the block `<<` now pushes either an Int or a Float). `floats.tally` needs
no driver — blockless, the interpreter path already beats YJIT (1.22×) and is parity-correct.

Hash-accumulator "sum-by-key" now ships too: `arr.each_with_object(Hash.new(0)) { |x, h| h[k]
+= v }` — the first JIT driver compiling in-block Hash read-modify-write. The block JIT models a
`HashObjId` memo with `[]` (read, Int default 0) and `[]=` (`CallAset`, write) on Int keys/values
(an Int element, or a Float element's `x.floor`); it accumulates into a fresh scratch Hash and
merges into the real memo only on success, gated to an empty `Hash.new(0)` (others decline). Both
Int and Float arrays drive it.

**Pure Float-KEY sum-by-key — SHIPPED**, 2.18× YJIT (jitN 0.60s vs 1.31s; 5.7× over interp).
`arr.each_with_object(Hash.new(0)) { |x, h| h[float_key] += int }` now compiles with a FLOAT bucket
key — the Float element used directly (`h[x] += 1`), or an Int element promoted (`h[x * 1.5] += 1`).
The `[]`/`[]=` block codegen branches on the key kind: an Int key keeps the exact-match Int-accum
primitives; a Float key bitcasts to its f64 bits and calls two new primitives
(`jit_hash_accum_get_floatkey` / `_set_floatkey`) whose bucket match **replicates `Value::ruby_eql`'s
Float arm** (NaN by bits — distinct NaNs never collide — every other Float by `==`, so -0.0 and 0.0
share a bucket), identical to the already-shipped `jit_group_push_floatkey`. The VALUE stays Int
(sum/count-by-key); no driver/dispatch/cache change was needed (key kind is per-op from the operand
stack, fixed per proto, so the existing `(proto_idx, float_elem)` cache key suffices). The margin is
smaller than the ~6× block-elimination drivers because both rubyrs and YJIT pay the per-element Hash
probe (the accum is an O(distinct-keys) linear scan); the JIT win here is only the eliminated block
dispatch. Parity vs CRuby (count, non-1 increment, Int→Float key, -0.0/0.0 bucket collapse, distinct
NaN, empty, first-appearance order) on interpreter == JIT; STRESS_GC clean (the scratch Hash is the
sole alloc, held live across the GC-free loop); diff_cruby GREEN both builds + a dedicated
`jit_float_key_sum_by_key` fixture.

**Float-VALUE sum-by-key — SHIPPED**, 2.17× YJIT (jitN 0.63s vs 1.37s; 6× over interp). A Float
accumulator per Hash key: `arr.each_with_object(Hash.new(0.0)) { |x, h| h[k] += float_v }` — e.g.
summing prices by category (`items.each_with_object(Hash.new(0.0)) { |it, h| h[it.cat] += it.price }`).
The Hash-accum codegen now decouples **two axes**: the KEY (Int exact-match vs Float eql?-match, picked
per-op from the operand stack) and the VALUE/accumulator (Int vs Float, driven by `float_acc` —
repurposed for `EachObjHash` since the Hash value IS the accumulator). The `[]`/`[]=` ops select one
of four primitives by `(key_kind, float_acc)`; a Float value travels as its f64 bits through the i64
ABI (default 0.0, bitcast to F64 on the operand stack so the `+= v` and the write treat it as Float).
The dispatch gate accepts `Hash.new(0)` (Int) or `Hash.new(+0.0)` (Float) and threads `float_val` into
the block-compile + cache key `(proto, float_elem, float_val)`. Int→Float coercion makes `h[k] += 1`
(Int literal) into a Float accumulator work via the existing `emit_numeric_binop`. Parity vs CRuby
(Int/Float key × Int/Float increment, floor-key, coercion, -0.0/0.0 collapse, empty) on interp == JIT;
STRESS_GC clean; diff_cruby GREEN both builds; the `jit_float_key_sum_by_key` fixture covers all combos.

The common Array + Hash Enumerable/aggregation surface is now broadly covered for Int and Float across
elements, keys, AND values.

### Layer 3g — B4 object-method dispatch: the do_call breakthrough (beats YJIT 3×, CRuby 4.8×)

The lever that finally cracks **`do_call`**. Fresh re-measurement (2026-06-29) confirmed the interpreter
dispatch floor is structural: even an *already-fast-pathed* fixed-arity object-method call is 2.5× CRuby,
and a variadic one 5×; the ADR-0031 binder frontier is exhausted; and the JIT did **not** fire on these
shapes (an object-method call *inside* a driver loop re-enters `do_call` per element). That is exactly B4.

**`objs.sum { |o| o.method(CONST) }` over a monomorphic Object array now compiles to a whole-loop native
driver — 3.0× YJIT (jitN 0.24s vs 0.72s), 4.8× CRuby (1.15s), 22× interp (5.34s).** The framework-dispatch
PoC predicted 4× YJIT (an idealized upper bound); the first real B4 driver hits 3× on a 1-arg-method shape.

Mechanism — **dispatch-time monomorphic specialization**, no runtime compilation in the loop:
- The array's FIRST element fixes the receiver class. The callee is resolved (`lookup_method_uncached`,
  method_gen-correct) and compiled for that class (`compile_native_for_class`, the existing B1 method JIT),
  and its native address + the class pointer + the constant Int arg are baked into a specialized loop.
- Per element: `jit_array_elem_obj_guarded` reads the element, requires `Value::Object` with the baked
  class (deopt otherwise), and returns a pointer to the element `Value` inside the array's backing Vec —
  the receiver for a **direct native→native call** to the compiled method (NO `do_call`, no frame, no
  dispatch). Overflow-checked accumulate; one shared `deopt` exit.
- Soundness: a compiled method is pure (the op-gate admits no side effect), so a part-way deopt commits
  nothing → the caller redoes the sum generically. A class miss / non-Object element / method-internal
  deopt / i64 overflow all deopt. The loop cache key is `(block_proto, class_ptr, callee_proto)` so a
  method redefinition (new proto) recompiles rather than calling a stale address. The element pointer is
  valid across the call: the array is pinned, the heap is non-moving, and the GC-free pure method does not
  realloc the array. STRESS_GC clean; diff_cruby GREEN both builds (947); `jit_objmethod_sum` fixture
  covers the polymorphic-deopt, bignum-overflow-deopt, non-Object-deopt, empty, and explicit-seed shapes.

This is the validated answer to "how to break through `do_call`": **not another interpreter cascade tweak
(frontier exhausted, gap structural) — bypass `do_call` with a native driver + a baked monomorphic class
guard.**

### Step 1 — general explicit-recv calls inside compiled bodies (the OO-dispatch lever)

A re-measurement sharpened the target. The shipped JIT already beats YJIT on recursion (2.8×) and on
`while`-loops + **self**-call chains *inside a method* (3.8× — `compile()` handles those); the earlier
"call chains lose to YJIT" was a top-level-`<main>` artifact. The one real gap for OO code is an
**explicit-recv call to ANOTHER object** (`@h.compute(x)`, recv ≠ self) inside a hot loop: `compile()`
had no Object-receiver `Call` arm, so the whole method declined (4.5× behind YJIT).

**SHIPPED** — generalize B4's native→native call from array-elements to **any (ivar) receiver**, via a
**runtime-fill monomorphic PIC**:
- `@h.compute(i)` (1-arg): jitN 3.35s → 0.33s, **2.4× AHEAD of YJIT** (was 4.5× behind), 16× interp.
- `@c.val` (0-arg, the canonical attribute/derived shape): jitN 4.30s → 0.28s, **2.1× AHEAD** (was 7×
  behind), 15× interp.

Mechanism: a new `Kind::Object` (a `*const Value`) is pushed by `LoadIvar` when `ivar_call_receiver` (a
bounded forward stack-effect walk — receiver and `Call` are not adjacent, the arg expr sits between)
detects the ivar is a call receiver. The explicit-recv `Call` arm lowers it to `jit_obj_call`: read recv
class; cache hit → call the cached native addr directly (no `do_call`/frame); empty → resolve + compile
the callee for that class (`jit_compile_obj_callee`, re-entrant via `&mut *vm` — same pattern as the
heap-mutating primitives; compile touches `jit_native`/`protos` not `heap`, runs no Ruby) + fill the PIC;
class miss → deopt. Gated to non-builtin, non-closure, Int-returning callees.

Two soundness results the coverage exposed (both fixed, both fixture-guarded):
1. **Null-receiver after a deopt.** The method codegen accumulates the deopt flag but does NOT branch
   (ops compute-and-discard on `ovf` — fine for arithmetic), so a preceding non-Object-ivar deopt leaves
   a NULL `recv` at `jit_obj_call` — which now null-checks before dereferencing. (Crashed `@int.eql?(o)`
   to empty output before.)
2. **Arity-swallow.** A 0-arg callee compiles only via `allow_zero_arg` (threaded through a dedicated
   `compile_native_for_class_zeroarg`, never the shared gate) AND is cached in a SEPARATE
   `jit_native_zeroarg` map — so a 0-arg `NativeProto` can never be served at a 1-arg dispatch site
   (B1 / explicit-recv read `jit_native`), which would swallow the wrong-arity `ArgumentError`
   (`cell.val(99)` must raise). Caught by the fixture (`:argerr` vs `:no_raise`) before the fix.

diff_cruby GREEN both builds (949); STRESS_GC clean (recv pinned, non-moving heap, GC-free callee);
`jit_obj_recv_call` fixture covers 1-arg + 0-arg fires, polymorphic deopt, bignum overflow, non-Object-ivar
deopt, empty, the arity-swallow guard, and implicit-self identity. North-star: `crates/rubyrs/benches/
jit_oo_dispatch.rb`.

**LOCAL receivers — SHIPPED.** `x = @h; … x.compute(i)` (an ivar cached in a local, then called): jitN
0.36s vs YJIT 0.75s = **2.1× AHEAD**, 15× interp. `StoreLocal` now carries a `Kind::Object` pointer and
`LoadIvar` loads `@h` as a pointer when it is stored to a local later used as a receiver
(`local_is_obj_receiver`). Sound: a compiled method never writes ivars (StoreIvar isn't modelled → it
declines), so the ivar table never reallocs and the stored pointer cannot dangle across the loop. This is
the substrate for param receivers (params are local slots).

**PARAM receiver — layer 1a SHIPPED (foundation, not yet a YJIT win).** Validation corrected a handoff
that framed the rubocop gap as "purely value-op coverage": the FIRST wall is the Object-arg ABI, not
value-ops. `bench_treesum` fires only because it is all-ivar-recv + all-int; a 1-Object-param method does
not fire (B1 only routes Int args). Layer 1a (commit 424a281d): a 1-arg method called with an Object arg
compiles a `compile_native_objparam` variant (param binds `Kind::Object`, i64 C-arg = `*const Value`);
B1 invokes it with a pointer to the arg on the operand stack; `LoadLocalCall` (`node.value`) lowers via
the obj-call PIC. It FIRES (~1.6× interp) but is dispatch-bound on a leaf-in-interpreted-loop (4.3× behind
YJIT) — committed as foundation. The YJIT-beating win is **layer 1b**: a compiled caller cross-calling with
the Object arg so param-receiver RECURSION (`walk(node)`→`walk(child)`) goes fully native like ivar-recv
treesum. The recursion arg comes from `kids[i]` (Array#[] returning an Object), so 1b interleaves with the
value-ops — the real layered climb to a firing `bench_walk` (poc/rubocop-spike/, the ultimate north-star,
which additionally needs symbol keys / Hash[sym] / Bool-as-control-flow). Also remaining: an N-way PIC
(+ Object branch-merge) for polymorphic sites; a varying call arg.

### Step 1 COMPLETE for the rubocop AST-walk skeleton — bench_decomp BEATS YJIT

Pieces 1–5 + "1a" + two micro-opts land the rubocop-shape recursive Object-tree walk
(`poc/rubocop-spike/bench_decomp.rb`, the cop-visitor skeleton) as native code that beats YJIT:
**jitN median 0.500ms vs YJIT median 0.524ms (~5% ahead, tighter distribution), 39× over interp** —
from DECLINED / 36× behind. The pieces: (1+2) Object-recv getter→Array (`jit_obj_getter_array`) +
local-array `length`/`[i]`; (3) heterogeneous `kids[i]` element pointer (`jit_array_elem_ptr`) + fused
`x.class == Const` guard (`jit_value_is_class`, const resolved via the flat `classes` table at runtime);
(4) Object-arg self-recursion; (5) the `stmt if cond` Int/Nil-merge-discard (`block_args` allows a
top-slot Nil/X mismatch iff the target block immediately `Pop`s it — sound: a USED merge still requires
exact match); "1a" the non-self Object-arg cross-call (`weigh(@h)` in a loop, 2.3× YJIT, via an
`obj_param_callees` map mirroring `float_callees`). Micro-opts past par: `Heap::class_ptr_of` (clone-free
class guard — `try_class_of` cloned the Rc just for its pointer) + a single `heap.get` in the getter-array
PIC. diff_cruby GREEN both builds (955), STRESS_GC clean, fixtures jit_obj_{array,tree}_walk /
jit_obj_arg_crosscall / jit_stmt_if_merge / jit_ivar_array_index / jit_objparam. Remaining rubocop
frontier: the FULL cop visitor (`bench_walk.rb`) needs the value-type axis (Hash[sym], Bool-as-control
over symbols); the top-level `<main>`-while-loop driver is still 0-win (needs `<main>` compilation).

### `bench_walk` pieces 1 + 8 — the 2-ARG method ABI + 2-arg recursion — SHIPPED

The FULL cop visitor `walk(node, counts)` (`poc/rubocop-spike/bench_walk.rb`) declined at the
very FIRST gate: the method arity gate admitted only 0/1 required positionals, so a 2-param
method never even reached codegen. This is the keystone of the 8-piece `bench_walk` set (which
is all-or-nothing — the method fires only when every piece lands). Pieces 1 (2-arg ABI) + 8
(2-arg Object+Hash recursion) are now done for the **Object + Int** param shape (the Hash 2nd
param is pieces 2–4, the value-type axis below):

- **`obj_param2`** compile mode: param0 binds `Kind::Object` (i64 C-arg = `*const Value`),
  param1 binds Int via the existing `two_param` plumbing (`arg3_slot = Some(1)`, a 4th i64
  C-ABI param). The arity gate admits `n == 2` iff `obj_param2`. A 2-arg self-recursive
  `CallNoRecv(name, 2)` lowers to a native 4-arg self-call (pop arg1 Int, arg0 Object ptr,
  call `self_ref`). Result is Int (the threaded accumulator).
- **`NativeProto::ptr2`** + **`call2`**: a 2-arg method's compiled code has a 4-param C
  signature, exposed under a distinct fn-ptr type (the 3-arg `call` would mis-pass the ABI).
- **Dispatch** (`try_invoke_fixed_method_from_stack`, new `argc == 2` arm): a no_recv / top-level
  `walk(node, acc)` with an Object 1st arg + Int 2nd arg compiles `compile_native_objparam2`
  (cached in `jit_native_objparam2`) and calls it with a pointer to the Object arg + the Int.
  Same already-resolved-`m` discipline as the 1-arg B1 path; a deopt or non-(Object,Int) shape
  falls through to the interpreter frame.

Measured (`poc/rubocop-spike/bench_walk2.rb`, the rung-A 2-arg tree walk, best-of-3 wall ms):
**jitN 0.506ms vs YJIT 0.517ms — ~1.02× ahead, 41× over interp (20.8ms), 6.8× over CRuby
(3.45ms)**. The shape is dispatch/recursion-bound (where YJIT is also strong); the value-type
axis is where rubyrs pulls further ahead. diff_cruby GREEN both builds (956), STRESS_GC clean,
fixture `jit_obj_arg2_walk` (heterogeneous children, polymorphic-`Sub`-no-recurse, depth, non-zero
seed). The proto is class-agnostic (empty callees/getters → no `guard_class`; the per-site PICs
handle each receiver class), and the receiver pointer is valid across the call (array pinned,
non-moving heap, GC-free pure method) — same soundness as the 1-arg obj-param path.

### `bench_walk` pieces 2 + 3 + 4 — `Kind::Symbol` + Symbol-keyed Hash read/write — SHIPPED

The value-type axis's substrate. `walk(node, counts)` reads the node's `type` Symbol and
accumulates a per-type count in the Hash param (`counts[t] = (counts[t] || 0) + 1`):

- **`Kind::Symbol`** — an i64 holding a `SymId.0`. Produced by `jit_obj_getter_sym` (an
  Object-recv getter whose ivar is a Symbol — `node.type` → `@type`, monomorphic class PIC,
  mirrors `jit_obj_getter_array`); chosen over the Int obj-call PIC when the getter result is
  stored to a local used as a Hash KEY (`local_is_symbol`). Flows through `JumpIfFalse` as a
  never-taken branch (a Symbol is always truthy).
- **Piece 2 (read + `|| 0`)** — a `Call([], 1)` with a `HashObjId` receiver + a `Kind::Symbol`
  key lowers to `jit_hash_get_symkey`, **gated on the following `Dup, JumpIfFalse, Jump, Pop,
  LoadConstInt` `|| const` idiom** (sound: the primitive returns `0` for an absent default-less
  key, which `|| 0` keeps — matching `nil → 0`; a bare `h[sym]` without the `||` declines so the
  interpreter preserves nil). `JumpIfFalse` now models an Int/Symbol condition (always truthy).
- **Piece 3 (write)** — a `CallAset([]=, 2)` with a `HashObjId` receiver + Symbol key + Int value
  lowers to `jit_hash_set_symkey`.
- **2-arg param1 = Hash** — `obj_param2` binds param1 as `HashObjId` when the body uses it as a
  `[]`/`[]=` receiver (`local_is_hash`); the dispatch passes the Hash `ObjId` and validates the
  runtime arg against the compiled `param2_hash`. The 2-arg self-recursion threads the Hash.
  Nil-returning methods box `Value::Nil` (`returns_nil`).

Measured (`poc/rubocop-spike/bench_walk3.rb`, best-of-3 wall ms): **jitN 0.95ms vs YJIT 1.28ms —
1.35× ahead, 24× over interp (23.3ms), 4.6× over CRuby (4.36ms)**. The decisive perf lever: the
Symbol-keyed Hash primitives do a **direct linear scan over `pairs`** (a handful of distinct keys)
rather than `ruby_hash` + the lazy FxHashMap index — index-free, the read/write at 2.34ms → 0.95ms.
diff_cruby GREEN both builds (957), STRESS_GC clean, fixture `jit_sym_hash_walk` (per-type counts,
polymorphic-`Sub`, depth, pre-seeded hash, defaulted-`Hash.new(10)` deopt). A latent disambiguation
bug fixed en route: `local_is_array`'s `[i]` arm matched the KEY position (`LoadLocal(slot),
Call([],1)`) — it now requires the RECEIVER position (`LoadLocal(slot), <key-load>, Call([],1)`),
so a Symbol hash-key local is no longer misread as an array.

### `bench_walk` pieces 5 + 6 + 7 — the FULL rubocop cop-visitor FIRES, beats YJIT 1.13×

The predicate axis completes `bench_walk` (`poc/rubocop-spike/bench_walk.rb`, the ultimate
north-star — the full cop visitor):

- **Piece 5 (Bool predicate obj-methods)** — `node.send_type?` (`@type == :send`) as a
  `JumpIfFalse` condition. A method whose `Return` is a Bool now compiles (`returns_bool`,
  the `icmp` 0/1). The obj-call site, when the result feeds a `JumpIfFalse`, calls
  `jit_obj_call_bool` (a PIC that accepts only a Bool callee) and pushes `Kind::Bool`. The
  dominant `@ivar == :sym` predicate is **inlined**: at PIC fill the callee bytecode is matched
  (`[LoadIvar, LoadSymbol, BinOp(Eq), Return]`) and a per-call hit does ONE heap fetch (class
  guard + ivar) and a compare — no native call. Needs `Kind::Symbol` ivar reads
  (`jit_ivar_get_sym`) + Symbol `==` (`emit_numeric_binop`).
- **Piece 6 (`is_a?(Class)` ancestry)** — `name.is_a?(Symbol)` / `c.is_a?(Node)`. Fused
  `LoadLocal[Object], LoadConst, Call(is_a?, 1)` → `jit_value_is_a` (ancestry via `class_is_a`,
  vs the exact `== class` guard). A 4-field cache: an Object whose EXACT class is the target
  short-circuits (no `class_of`/walk); a non-Object value (a Symbol) memoizes the answer by its
  type tag (its class is fixed) — so the per-kid `c.is_a?(Node)` and per-send `name.is_a?(Symbol)`
  are O(1) after warmup.
- **Piece 7 (`Symbol#length`)** — `name.length` on a heterogeneous element pointer →
  `jit_symbol_length` (deopt if not a Symbol).
- The heterogeneous `name = children[1]` is read as a pointer (`local_is_obj_value` extended to
  is_a?/length uses); `node.children[i]` / `node.children.length` chained off the getter yield an
  Array (`arr_result` extended to the chained `[i]` / `.length` forms); the nested `if/elsif/end`
  statement value (Int-or-Nil through several jump blocks before a `Pop`) merges via a generalized
  Nil-downgrade in `block_args` (a "discard poison" the only ops accepting are Pop / further merge).

**Soundness fix (also repairs rung B, shipped buggy):** a compiled `walk` MUTATES its Hash param
(`counts[t] = …`, a side effect), so a deopt-after-write would leak the native run's writes and
double-count on the interpreter redo (verified: `{lit: 6, send: 2, if: 2}` vs CRuby `{lit: "x",
send: 1, if: 1}`). Fix: the Hash-param dispatch runs the native walk against a **pooled, GC-rooted
SCRATCH** (a re-seeded clone of `counts`), committing its pairs back into `counts` only on full
native success and discarding on deopt — so the interpreter re-runs on the untouched original. The
scratch is pooled (alloc'd once) so the common path adds no allocation.

**Perf lever — the hash read-modify-write FUSION.** Profiling the firing walk showed the
Symbol-keyed Hash `counts[t] = (counts[t] || 0) + 1` (two `heap.get`s + two scans + the `|| 0`
control flow, per node) was the #1 cost. A compile pre-pass detects the exact r-m-w op range and
fuses it into ONE `jit_hash_incr_symkey` (one heap access, find-or-insert + add); the leader pass
excludes the range's interior so the `|| 0`'s internal jump blocks are never emitted. Result:

| | jitN | YJIT | CRuby |
|-|-----:|-----:|------:|
| `bench_walk` per_iter | **1.22ms** | 1.39ms | 5.27ms |

**jitN 1.13× ahead of YJIT** (was DECLINED / ~19× behind), 4.3× CRuby, ~22× interp. STRESS_GC
clean, diff_cruby GREEN both builds (957), fixture `jit_sym_hash_walk` (predicate, is_a?, length,
pre-seeded, defaulted-hash deopt, and the overflow-deopt-after-write soundness case). A latent
**pre-existing interpreter bug** surfaced while testing: `IncLocal` (`x += 1`) WRAPS on i64
overflow instead of promoting to Bignum (`BinOpInt` + `BinOp` promote correctly) — orthogonal,
noted for a separate fix.

The full rubocop AST-walk (`bench_walk`, all 8 pieces) now runs native and beats YJIT.

### Two remaining gaps (2026-06-29)

**Gap A — the `.each`-form walk — SHIPPED (beats YJIT 1.5×).** Real rubocop cops walk via
`node.children.each { |c| … }` / `each_child_node`, not `while`. Previously the `each` form
declined (the method's `CallBlock(each)` op isn't modelled) and was a REGRESSION (jitN
21.5ms vs interp 17.6ms). Fix: **`rewrite_each_block` rewrites `recv.each { |x| body }` into
the proven `while`-loop bytecode** before compiling — the block body IS the while-loop body
(it SHARES the method frame: the accumulator is a captured slot, `x` the param), so the
spliced ops compile exactly like the firing `while` form, with a recursive `walk(x)` a
native self-call. The block body is spliced verbatim (its relative jumps are
self-contained), wrapped in a synthesized index loop (3 temp slots: array/index/length),
and method jumps crossing the splice are remapped (or it declines). `compile` gained
`code`/`n_locals` overrides (`Proto` isn't `Clone`). A second fix was needed: `jit_should_route`
now keeps a 1-arg method routed to B1 until its OBJECT-param variant is also ruled out
(the obj-param compile lives inside B1; an Int/value-declined-but-obj-param-viable method
stopped routing before its variant ever compiled). Measured (`poc/rubocop-spike/bench_each_walk.rb`):
**jitN 0.51ms vs YJIT 0.78ms — 1.5× ahead, 33× interp** — identical to the `while` form (same
native code; profile confirms `do_call` negligible = recursion native, hot leaves are the
same value-primitives). diff_cruby GREEN both builds (958), STRESS_GC clean, fixture
`jit_each_obj_walk` (heterogeneous children, polymorphic-`Sub`, depth, empty-children).

**Gap A extended — the FULL cop body via `.each` (2-arg) — SHIPPED.** The first `.each` win
was narrow (a 1-arg simple body). The real rubocop shape is `walk(node, counts)` — a 2-arg
method whose `.each` block recurses `walk(c, counts)` AND whose body does Hash counting /
Symbol / `is_a?` / a Bool predicate / `&&`. `bench_walk_blocks.rb` declined (28ms) where the
`while` form fired (1.27ms). Three fixes:
1. **Wire the each-rewrite into the 2-arg path** (`compile_native_objparam2`, not just the
   1-arg `compile_native_objparam`) — the block's `walk(c, counts)` is a 2-arg native self-call.
2. **`.each` as the method's last expression** — the rewrite no longer requires a trailing
   `Pop`; it consumes just `[CreateBlock, CallBlock]` and re-pushes the receiver Array (`each`
   returns its receiver), so a following `Pop` (statement) OR `Return` (the cop body, where
   `.each` is the last expr) both work.
3. **Per-kind block-param types** (the general bug): `block_args` declared every basic-block
   param `i64`, but a Bool is an `i8` (`icmp` result) and a Float an `f64`. The `&&`
   (`name.is_a?(Symbol) && name.length > 8`) Dups a Bool ACROSS a block edge — the first time
   a Bool crosses one — tripping the Cranelift verifier (`i8` arg into an `i64` param). Params
   are now typed per kind (Bool→i8, Float→f64, else i64); the Nil-downgrade is gated on
   matching types.

`bench_walk_blocks.rb`: **jitN 1.22ms vs YJIT 1.71ms — 1.4× ahead, 20× interp** — identical to
the `while`-form `bench_walk` (1.22ms). diff_cruby GREEN both builds (960), STRESS_GC clean,
fixture `jit_each_cop_walk` (Hash/Symbol/is_a?/`&&`/predicate via `.each`, 2-arg recursion, depth).

**Gap B — `treesum` fires but trails YJIT 1.6×.** The double-recursion `@v + @l.sum(d-1) +
@r.sum(d-1)` is dominated by the per-node `heap.get` slab indirection: 3 self-ivar reads
(`@v`/`@l`/`@r`) + 2 child class-guards (`@l`/`@r`). **Shipped: an ivar-receiver call FUSION**
(`Kind::ObjectIvar` defers the ivar fetch so `@l.sum(d-1)` emits ONE `jit_ivar_obj_call` —
fetch ivar + class guard + native call — instead of `jit_ivar_obj_ptr` + `jit_obj_call`):
0.328ms → 0.30ms (8%, diff_cruby GREEN 957, no regression on the firing benches). But it
still trails YJIT 0.19ms ~1.6×. The 3 redundant self-`heap.get`s could be cut by caching
self's `Instance` pointer once per frame (~25%), but the 2 child class-guard `heap.get`s are
inherent: YJIT reads an object's class from its header in one load, while rubyrs's
`ObjId → slab slot → HeapObj::Instance` costs an extra indirection per guard. A clear treesum
surpass therefore needs an object-header class (a heap-representation change) — out of scope
here; the realistic dispatch shapes (`fib`, the OO north-star, `bench_walk`) already surpass.

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
