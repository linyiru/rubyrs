# Inline-cache hit-rate baseline

First measurement using the `ic-stats` cargo feature (PR #170)
against a fixed 5-workload battery covering the polymorphic IC's
design points. Numbers are from `perf/ic_stats.sh`, release
build, captured 2026-05.

Regenerate with:

```
cargo build -p rubyrs --features ic-stats --release
perf/ic_stats.sh
```

## Results

| Workload | Hits | Misses | Toplevel hits | Toplevel misses | Hit rate |
|---|---:|---:|---:|---:|---:|
| 01 monomorphic | 9 999 | 5 | 0 | 3 | **0.9992** |
| 02 polymorphic, 4 shapes | 19 995 | 9 | 0 | 3 | **0.9994** |
| 03 megamorphic, 5 shapes | 9 999 | 10 005 | 0 | 3 | **0.4998** |
| 04 hot toplevel def | 0 | 4 | 9 999 | 4 | **0.9992** |
| 05 DefMethod gen-bump churn | 0 | 1 004 | 0 | 3 | **0.0000** |

## What each workload measures

**01 — monomorphic.** Single class shape on the hot dispatch
site. Saturates the IC after the first miss. Baseline for "best
case".

**02 — 4-shape polymorphic.** Cycles among exactly `IC_WAYS = 4`
classes. All four ways fill on the first cycle, every subsequent
dispatch hits. Total hits ≈ 2 × N because both `.tag` (Object
dispatch on the polymorphic site) AND `shapes[i % 4]` contribute
IC accounting: `do_call` always consults `lookup_method_cached`
first, including for `Array#[]`, and caches the (None) result —
so Array indexing is a monomorphic IC hit on every iteration even
though the actual indexing runs through `collection_call`'s
fallback. **Confirms IC_WAYS = 4 is exactly sized for typical
4-shape polymorphism**.

**03 — 5-shape megamorphic.** One shape past `IC_WAYS`. The
round-robin eviction (`next_way`) keeps replacing a way that the
NEXT iteration will need, so every shape misses on a ~5-iteration
cycle. Hit rate collapses to ~0.5 — `shapes[i % 5]` still hits
(Array indexing is monomorphic, see the workload 02 note on the
negative-cache slot), but the `.tag` site misses on every
iteration. **This is the workload that would benefit from widening
IC_WAYS to 5 (or switching to LRU eviction from round-robin).**

**04 — hot toplevel def.** 10 000 calls to a user toplevel `def
helper`. Implicit-self routing through
`lookup_toplevel_method_cache_hit` (the fast path). The
toplevel-hit counter behaves identically to the receiver one —
**confirms PR #170's fast-path instrumentation fix**.

**05 — DefMethod gen-bump churn.** `Op::DefMethod` bumps a
GLOBAL `method_gen`, invalidating every cached entry across the
program. A redef-in-hot-loop pattern gets 0 hits because every
iteration the next dispatch finds a stale `generation` field and
re-walks. **Suggests a per-class generation (rather than a single
global) would be a worthwhile follow-up if any real workload
shows this pattern**, though no production fixture in the corpus
has been observed redefining methods in a hot loop.

## Calibration notes

- All hit rates are aggregate (`hit_rate()` = (hits + toplevel_hits) / total).
- The 4 fixed misses each workload reports are the preamble — class
  registration eval at Runtime construction time runs through a few
  cache-miss paths before user code starts. These are constant across
  workloads and dilute the headline rate only for short programs.
- `puts` and similar builtins go through a builtin-name fast path that
  does NOT touch the IC, so the workload's `puts total` at the end
  doesn't show up in either counter.
- Primitive dispatch paths split into two camps for IC accounting:
  - **Inline arithmetic / inline ops** (`Integer#+`, `<`, `==`, ...)
    are compiled to dedicated opcodes that don't go through `do_call`
    at all, so they never touch `lookup_method_cached`.
  - **Generic method calls on a primitive receiver** (`arr[i]`,
    `str.length`, ...) DO enter `do_call`, which calls
    `lookup_method_cached` first; the lookup returns `None` (no
    user-defined override) and the dispatcher falls through to
    `collection_call` / `primitive_call`. The IC-hit counter
    still increments because the cache stores the `None` result
    and subsequent calls match the cached entry. Empirically this
    means a hot `arr[i]` in a loop contributes ~N IC hits on top
    of the actual fast-path work.

## What this says about IC sizing

- **Mono and 4-way poly are optimal.** No work needed.
- **Megamorphic at exactly 5 shapes is the cliff.** If a future
  workload shows a 5–8 shape hot site (e.g. an `Enumerable` chain
  dispatching over a heterogeneous collection), widen `IC_WAYS` to
  6–8 or switch to LRU. The branch-prediction cost of widening is
  small for in-cache scans below ~8.
- **Gen-bump churn is total.** Worth file-watching for any future
  hot-path workload that hot-redefines methods (testing frameworks,
  metaprogramming-heavy code). Switching to per-class generation
  would be an O(N classes) memory cost for an unknown payoff —
  defer until a real workload demands it.
