# Perf budget

A per-workload **peak-RSS** ratchet. Same idea as
`.github/workflows/ci.yml`'s panic budget: a small text file holds the
expected ceilings, CI fails when a workload regresses, and intentional
bumps require an explicit comment in the file.

## Why RSS, not wall time

rubyrs' headline distinction over CRuby is **memory** (the README quotes
"~5× lighter than CRuby on the same workload"). RSS is the metric that
directly defends that claim. It's also <5% noise across runs on the
same machine — easy to threshold without flakiness.

Wall time is collected and printed by `perf/check.sh` (so reviewers can
spot a 30%+ slowdown), but **not gated**. CI runners vary by 2× even
within a single provider, which makes absolute wall-time thresholds
either too tight (constant noise) or too loose (catches nothing).

A relative wall-time check (this PR vs. master baseline, same runner)
is a meaningful follow-up. Not done in this first cut.

## Workloads

| Workload | What it stresses |
|---|---|
| `crates/rubyrs/tests/fixtures/fizzbuzz.rb` | Tiny script — RSS floor of the interpreter |
| `crates/rubyrs/examples/metaprog_bench/mm_bench.rb` | 2M `method_missing` dispatches; closure-allocator load |
| `crates/rubyrs/examples/metaprog_bench/dm_bench.rb` | 2M calls into a `define_method`-installed closure-method |
| `crates/rubyrs/examples/metaprog_bench/static_bench.rb` | 2M `def`+`@ivar` calls — control for the metaprog comparisons |

The three metaprog workloads are deliberate: they exercise different
allocation patterns (instance-method dispatch frames, closure
re-entry, ivar table write). A single broad regression should show up
in at least one; a targeted regression (only ivars, only closures)
shows up in exactly one.

## Running locally

```bash
cargo build --release -p rubyrs
perf/check.sh
```

The script honours `RUNS` (default 3), `BASELINES`
(default `perf/baselines.tsv`), and `RUBYRS_BIN`
(default `target/release/rubyrs`).

## Bumping a budget

Same etiquette as the panic budget:

1. **Add a comment line above the row** explaining *what allocation
   grew* and *whether it's an explicit design choice*. "Bumped because
   the new feature X allocates Y" is fine. "Bumped to make CI green"
   is not — investigate first.
2. **Don't lower a budget silently.** Lowering *with a comment*
   ("workload X now consistently runs at Y KB after Z's optimisation,
   tightening from N → M") is the right move when a workload genuinely
   got lighter. Lowering in a drive-by edit without a comment erases
   the historical ceiling that documents how we got here.
3. **Don't bump a budget to absorb a regression you didn't intend**.
   If the regression isn't part of the PR's stated change, treat it as
   a separate bug.

## Initial calibration

The first commit set every workload's budget to 8192 KB — roughly 2×
the local M-series Mac measurements (~2.5-2.7 MB). The headroom
absorbs GitHub-runner variance during the first few green runs. Once
the CI runner's actual ceiling is known, a follow-up PR should
tighten the budgets toward `measured + ~20%`. Until that ratcheting
pass, the budget catches *obvious* regressions (2×+ memory blowups)
but won't catch a steady 10%-per-PR drift.
