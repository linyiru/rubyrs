# Perf budget

A per-workload **peak-RSS + wall-time** ratchet. Same idea as
`.github/workflows/ci.yml`'s panic budget: a small text file holds the
expected ceilings for each metric, CI fails when a workload regresses,
and intentional bumps require an explicit comment in the file.

## What's defended

| Metric | Why it matters | Noise floor |
|---|---|---|
| Peak RSS (KB) | The headline "rubyrs is ~2× lighter than CRuby" claim ([SUBSET.md](../docs/SUBSET.md), [ADR 0010](../docs/adr/0010-metaprogramming-poc.md)). docs/BENCHMARKS.md "Memory" shows fizzbuzz at 4.7 MB vs CRuby 3.4.1's 10.3 MB (2026-06-11; was 2.1 vs 18.4 on the pre-Jekyll-era binary and an older CRuby build). | ≤5% on the same machine |
| Wall time (ms) | Dispatch-loop overhead; the second-most-visible cost. ADR 0010's bench shows we're currently 3× CRuby per iteration — `static_bench` etc. lock that gap in. | ~20-30% across CI runs |

RSS is the tighter gate (≤5% noise tolerates a snug budget); wall has
~1.5× headroom over the observed minimum to absorb the ~20-30% noise
floor with margin for occasional outliers. The `max_wall_ms` column
accepts `0` to disable the wall check on a workload — used for
sub-100ms scripts where measurement noise dominates the signal
(fizzbuzz).

## Why absolute baselines, not master-relative

An earlier sketch proposed comparing each PR against `master` HEAD on
the same runner ("PR's wall time must be ≤ 1.1× master's"). That
sounds tighter but **chain-degrades**: if every PR is within 10% of
"master at that moment", cumulative drift across N PRs is unbounded.
Absolute baselines in `perf/baselines.tsv` are the same shape as the
panic budget — explicit, reviewable, committed to source. Periodic
re-cal commits are expected when GitHub bumps the ubuntu-latest image
or rubyrs makes a real perf improvement.

## Workloads

| Workload | What it stresses |
|---|---|
| `crates/rubyrs/tests/fixtures/fizzbuzz.rb` | Tiny script — RSS floor; wall disabled |
| `crates/rubyrs/examples/metaprog_bench/mm_bench.rb` | 2M `method_missing` dispatches; closure-allocator load |
| `crates/rubyrs/examples/metaprog_bench/dm_bench.rb` | 2M calls into a `define_method`-installed closure-method |
| `crates/rubyrs/examples/metaprog_bench/static_bench.rb` | 2M `def`+`@ivar` calls — control for the metaprog comparisons |
| `crates/rubyrs/benches/fizzbuzz_1m.rb` | 1M-iter fizzbuzz — Op::BinOpInt + Op::IncLocal + dispatch fast paths |
| `crates/rubyrs/benches/gc_churn.rb` | 200k short-lived Array+Hash allocs — mark/sweep + `maybe_gc` heuristic |
| `crates/rubyrs/benches/string_interp.rb` | 200k `InterpolatedStr` builds — `Op::InterpStr` + `to_s` dispatch |
| `crates/rubyrs/benches/hash_mutate.rb` | 200k Hash `[]=` overwrites + 1k reads — `ruby_eq` Hash key probe |

The three metaprog workloads are deliberate: they exercise different
allocation + dispatch patterns. A broad regression shows up in all
three; a targeted one (only ivars, only closures, only dispatch
loops) shows up in exactly one.

The four `benches/*.rb` files were added to broaden the shapes
defended by the budget. The original four exercised
allocation-light dispatch; the new four add a 1M-iter
arithmetic loop (fizzbuzz_1m), GC pressure (gc_churn),
interpolation-heavy work (string_interp), and Hash mutation
(hash_mutate). A regression in one optimisation (e.g.
String#+ → Op::ConcatStr fusion) now shows up in the matching
workload directly.

## Running locally

```bash
cargo build --release -p rubyrs
perf/check.sh
```

Env vars: `RUNS` (default 3), `BASELINES` (default
`perf/baselines.tsv`), `RUBYRS_BIN` (default `target/release/rubyrs`).

## Bumping a budget

Same etiquette for both columns:

1. **Add a comment line above the row** explaining *what grew* and
   *whether it's an explicit design choice*. "Bumped because feature
   X allocates Y / adds Z ms" is fine. "Bumped to make CI green" is
   not — investigate first.
2. **Don't lower a budget silently.** Lowering *with a comment*
   ("workload X now consistently runs at Y after Z's optimisation,
   tightening from N → M") is the right move when a workload genuinely
   got cheaper. Drive-by lowers erase historical ceilings.
3. **Don't bump a budget to absorb a regression you didn't intend.**
   If the regression isn't part of the PR's stated change, treat it
   as a separate bug.

## Status semantics

CI prints one of:

- `ok` — both metrics under budget
- `RSS-OVER` — peak RSS exceeded `max_rss_kb`
- `WALL-OVER` — wall-time exceeded `max_wall_ms` (and the wall gate
  wasn't disabled)
- `RSS-OVER+WALL` — both exceeded
- `SETUP` — per-workload measurement failed (workload exited
  non-zero, `/usr/bin/time` output didn't parse, malformed
  `baselines.tsv` row, or workload path missing). Routes to
  exit 2, not exit 1. Note: a missing `/usr/bin/time` binary
  itself is a *global* setup error caught in `perf/check.sh`'s
  pre-flight before any per-workload row prints — it exits
  with the same code (2) but you'll see only the binary error
  message, no `SETUP` row.

## Calibration history

| Date | Change | Reason |
|---|---|---|
| (initial PR #11) | All four workloads: RSS 8192 KB, no wall gate | First commit; ~2× CI observation as flake margin while we calibrated. |
| (this PR) | RSS 8192 → 5500 KB; added wall column (mm 1100, dm 700, static 1000, fizzbuzz disabled) | Multiple green CI runs established ubuntu's real ceiling (~4.2-4.3 MB). Wall numbers from same runs, padded ~1.5× for runner variance. |

## Feature sanity guard (`jekyll_guard.sh`)

Run `perf/jekyll_guard.sh` BEFORE timing any Jekyll workload. Any
`cargo build` / `cargo test --release` without the full feature set
silently overwrites `target/release/rubyrs` with a default-feature
binary, and measurements against it are garbage (the classic symptom:
`Set#include?` NoMethodError aborts the build in ~0.06s and looks like
a miraculous speedup). The guard probes each feature's fingerprint
(host fns / stdlib Set) and fails loudly with the rebuild command.

## Allocator fingerprint (`alloc_fingerprint.sh`)

The same clobber trap applies to the allocator alone: a binary built
without `mimalloc` runs the system allocator, which no shipped CLI
does and which understates it by 2–19% depending on allocation
intensity (see docs/BENCHMARKS.md "Standard measurement feature
set"). `perf/alloc_fingerprint.sh [BIN]` verifies mimalloc is linked
(nm mi_ symbols, strings fallback for stripped binaries; macOS +
Linux) and exits non-zero with the rebuild command if not. Run it
before timing any binary that isn't covered by `jekyll_guard.sh`.
