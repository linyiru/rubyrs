# 0031: do_call dispatch-core optimization

## Status

Proposed / project kickoff (2026-06). Measurement-driven, incremental.
Follows the Sinatra CRuby-parity perf campaign (ADR-adjacent; see the
perf memory notes) that took a real Sinatra `GET "/"` request from
483µs → 110µs (4.4×, ~28× → 6.6× vs CRuby) via two concentrated fixes
(skip backtrace materialization for `throw`/`catch`; cache `getcwd` for
`Dir.pwd`/`File.expand_path`). After those, the remaining 6.6× gap is
**distributed VM-maturity floor**, and a deterministic sweep
(instructions-retired) showed no easy ≥0.3% micro-fish remain — the
interpreter's simple opcodes are already LLVM-optimized. The cost is
concentrated in **method dispatch**.

## Context — the measurements that frame this

Workload: the gem-probe Sinatra app, `boot + 1000 GET "/"` requests,
measured with `/usr/bin/time -l` (instructions retired = deterministic,
Δ<0.05%; the primary tool — wall-clock noise hides sub-1% changes).

1. **Opcode histogram** (2.08M ops): the call family dominates the
   instruction cost — `Call` 11.6% + `CallNoRecv` 5.2% + `LoadLocalCall`
   2.0% + `CallBlock`/`CallNoRecvBlock`/`CallAset` ≈ **~25% of all ops**.
   The simple ops (LoadLocal 13.7%, Return 10.5%, JumpIfFalse 8.0%, …)
   are already tight: unchecked LoadLocal saved only −0.08% (LLVM already
   elides the bounds checks). So the lever is `do_call`, not the basics.

2. **do_call fast-path coverage** (434,837 calls): `do_call` already has
   a fast-path chain — proc.call → `try_fast_primitive` → `try_fast_index`
   → `try_invoke_explicit_recv_cached` → `try_invoke_class_singleton_cached`
   → slow cascade. **But 72.4% of calls fall through ALL of them into the
   slow cascade.** Breakdown of the fall-through:
   - **25.7% of all calls are `no_recv`** (implicit self, `foo`/`self.foo`
     inside a method or route block). The explicit-recv fast path gates
     on `!no_recv`; the toplevel no_recv fast path only handles
     `main`/`nil` self — so implicit-self calls on an Object self (every
     Sinatra helper/DSL call inside a route) miss everything.
   - **~46.7% are explicit-recv calls that miss
     `try_invoke_explicit_recv_cached`** — which requires Object receiver +
     cached + public + **fixed-arity** + **non-closure**. Sinatra/Rack
     methods routinely take blocks / `*args` / optional params, so they
     fall through despite being perfectly cacheable.

## Decision

Widen `do_call`'s fast-path coverage so the common Sinatra call shapes
stop falling into the slow cascade. Measurement-driven and incremental;
every increment is independently committable, passes the full diff +
lib suites, and is A/B'd in **instructions-retired** (≥0.3% bar; also
check cycles + wall — wall is the arbiter, instructions miss
memory-bound effects).

### Increments (prioritized by the measured distribution)

1. **`no_recv` cached fast path for an Object self** (~25.7% of calls).
   When `no_recv` and the current frame's `self_val` is a `Value::Object`,
   resolve via the same inline cache (`cache_id`) the explicit path uses
   and invoke stack-direct. Mirror `try_invoke_explicit_recv_cached`'s
   soundness gates (public-or-self-call, fixed-arity, non-closure,
   `!force_primitive`, `!maybe_refined`). Implicit-self calls are
   private-callable, which simplifies the visibility gate.

2. **Extend the cached fast path to block-taking / non-fixed-arity
   methods** (~46.7%). The current gate is conservative (fixed-arity,
   non-closure). Many cacheable methods miss only because they accept a
   block or optional/splat args. Widen the arg-binding fast path to cover
   them (this is the larger but harder win — arg binding for
   optional/splat/block is where the complexity lives).

3. **(Later) Single receiver-type dispatch** instead of sequential
   try-and-miss. The fast paths each re-derive `ridx` and re-examine the
   receiver; a single `match` on the receiver Value at the top, routing
   to the right handler, removes the redundant re-examination across the
   chain.

## Risks & guardrails

- `do_call` is the hottest + most correctness-critical function. **Codegen
  ripple** is real (this campaign repeatedly saw ±50-90ns / few-% swings
  from unrelated layout shifts). Mitigation: extract new fast paths into
  `#[inline(never)] fn try_*` helpers (the `op_new_hash` pattern that
  landed a clean win without rippling `step()`), and A/B every change.
- **Correctness**: each increment must pass the full `diff_cruby` suite +
  lib tests before commit. The fast path MUST resolve identically to the
  slow path (same `class_of` + `lookup_method_cached`); divergence =
  silent wrong dispatch. Add diff fixtures for the shapes each increment
  newly fast-paths (implicit-self private call, block-taking call, etc.).
- **Visibility**: implicit-self calls may invoke private methods (correct);
  explicit-recv may not. The no_recv fast path must encode that asymmetry.

## Expected payoff

If increment 1 fast-paths the 25.7% no_recv calls and increment 2 reaches
a meaningful slice of the 46.7%, the slow-cascade share drops from 72% to
a fraction, cutting the per-call instruction cost across ~25% of all ops.
Order-of-magnitude target: a few % of the Sinatra request each — chained,
a double-digit-% dent in the 6.6× gap. To be confirmed per-increment by
measurement; no increment ships without a measured ≥0.3% wall win.

## Results (2026-06-24)

- **Increment 1 (no_recv fixed-arity fast path) — SHIPPED, −2.3% wall**
  (commit `8c4be14b`). Resolves implicit-self calls on an Object self via
  the cached fast path, stack-direct. Correctness: diff 1001/0 +
  norecv_self_dispatch fixture. CRITICAL MEASUREMENT LESSON: this path is
  **+15.6% instructions-retired yet −2.3% wall / −1.7% cycles** — the
  extra (cheap, predicted) instructions buy avoidance of the slow
  cascade's memory-stally work. It was nearly discarded on the
  instruction count alone. **Instructions-retired is MISLEADING for
  dispatch/memory-pattern changes; wall + cycles are the arbiters.**

- **Increment 2 (explicit-recv variadic via invoke_method) — REJECTED,
  +9.7% wall / +8.3% cycles** (reverted). Falling back to the general
  binder for non-fixed-arity calls regressed (both wall AND cycles up —
  a real regression). PRINCIPLE: the cascade-skip only pays off when the
  invoke is ALSO cheap (stack-direct, arena, no Vec). Variadic calls need
  the expensive general binder (+ a `drain.collect()` Vec / pre-empting a
  cheaper slow-path route), which the cascade-skip can't offset.

## Spun off: faster general binder (scoped, NOT yet implemented)

The variadic slice (~46.7% of slow-cascade calls) is binder-bound. The
binder is `invoke_method_with_block_inner`'s non-closure path
(dispatch.rs ~15632+). Per-call costs identified:
  - `vec_nil(n_locals)` — a FRESH heap locals Vec per variadic call
    (~51/request), where the fixed-arity path reuses the locals ARENA.
  - `kw_param_defaults.clone()` + `kw_has_computed_default.clone()` —
    snapshot clones per call (to release the `self.protos` borrow before
    the rest-Array `heap.alloc`); cheap when kw_count==0, a real clone
    otherwise.
Proposed: route stack-eligible variadic protos through the arena for
their locals buffer (avoid the per-call Vec). CHALLENGE: the binder
writes `locals` by index INTERLEAVED with `heap.alloc` for the `*rest`
Array under PinGuard GC-root guards — holding an arena borrow across
those allocs is a borrow conflict, so this is an intricate refactor in
the most correctness-critical function (wrong arg binding = silent wrong
behavior across all Ruby code). Warrants a dedicated, fresh-context
effort with full diff-suite gating, NOT a tail-of-session change.
