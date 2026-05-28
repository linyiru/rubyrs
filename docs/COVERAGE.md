# Coverage ratchet

Per-file line-coverage gate, same shape as the panic-budget and perf-budget
ratchets: baselines live in-tree, CI fails on regression, intentional drops
are reviewed via the baseline JSON diff in the PR.

## What's measured

`cargo llvm-cov --workspace --all-targets --lcov` line coverage, filtered to
files under `crates/`. Third-party deps and integration-test files are
excluded by the ratchet script (`scripts/coverage_ratchet.py`).

Per-file baselines live in `crates/rubyrs/coverage_baseline.json`. Values
are **whole percentage points** (rounded down from the actual measurement
when captured) so sub-1% noise doesn't flap the gate. The `tolerance_pct`
field gives an additional 1% leeway, so a file at baseline `78%` passes
anywhere from `77.0%` upward.

## Running locally

```sh
# One-shot install
cargo install cargo-llvm-cov --locked

# Measure
cargo llvm-cov --workspace --all-targets --lcov --output-path lcov.info

# Check against baseline (same call CI makes)
python3 scripts/coverage_ratchet.py \
    --lcov lcov.info \
    --baseline crates/rubyrs/coverage_baseline.json
```

The HTML report (handy for finding uncovered lines) is one extra flag:

```sh
cargo llvm-cov --workspace --all-targets --html
open target/llvm-cov/html/index.html
```

## When CI fails

Two common shapes:

### "Coverage regressions"

A file dropped below `baseline - tolerance_pct`. Two valid responses:

1. **Add tests.** Recover the coverage so the baseline holds. Use the HTML
   report to find the uncovered lines.
2. **Lower the baseline.** Sometimes a refactor adds production code faster
   than tests can keep up (planned follow-up tests, deferred error-path
   tests, etc.). Rerun:

   ```sh
   python3 scripts/coverage_ratchet.py \
       --lcov lcov.info \
       --baseline crates/rubyrs/coverage_baseline.json \
       --update
   ```

   The baseline diff lands in your PR — explain the drop in the PR body so
   the review captures the trade-off explicitly. **Don't lower silently**;
   the entire point of the ratchet is that drops are visible.

### "Source files without coverage baselines"

A new source file was added but no baseline entry exists. Run `--update`
to capture its measured % into the baseline (same command as above).

## What's excluded and why

- `crates/*/tests/**` — test files themselves aren't covered by their own
  tests. (cargo-llvm-cov measures coverage OF library code BY tests; the
  test files don't appear in the LCOV output, but the script also defends
  against any that slip through.)
- Third-party deps (`registry/`, git deps) — cargo-llvm-cov emits these
  when `--workspace` pulls them; the script filters to `crates/` only.
- Generated code: none currently. If we add build-script-generated source
  in the future, exclude via `.cargo/config.toml` `[env]` overrides or by
  filename match in the ratchet script.

## Why not a single whole-crate % gate?

The panic-budget precedent is per-file. Coverage works the same way: a
regression in one hot file (say `vm/step.rs` losing 5%) is invisible if the
whole-crate average barely moves. Per-file forces every file's coverage to
hold or improve.

## See also

- `.github/workflows/ci.yml` (`coverage` job)
- `scripts/coverage_ratchet.py`
- `crates/rubyrs/coverage_baseline.json`
- `docs/PANIC_AUDIT.md` — sibling ratchet (panic budget)
- `docs/TESTING.md` — overall testing strategy
