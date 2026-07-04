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
# One-shot install — pin matches the CI version so local + CI
# measurements use the same tool. When bumping, also update
# `CARGO_LLVM_COV_VERSION` in `.github/workflows/ci.yml` (the
# coverage job's source of truth); this line below is a
# manual-sync copy for copy-pasteability.
cargo install cargo-llvm-cov --locked --version 0.8.7

# Measure. RUST_MIN_STACK matches the CI coverage job: the
# debug+instrumented build's frames are 2-3x release and the
# preamble-compile unit tests overflow default 2 MB test threads.
RUST_MIN_STACK=16777216 \
cargo llvm-cov --workspace --all-targets --lcov --output-path lcov.info

# Check against baseline (same call CI makes)
python3 scripts/coverage_ratchet.py \
    --lcov lcov.info \
    --baseline crates/rubyrs/coverage_baseline.json
```

The pinned version is declared once in the coverage job's
`env: CARGO_LLVM_COV_VERSION` block in
`.github/workflows/ci.yml`, then referenced by both the
`install-pinned-cargo-tool` composite invocation and the
instrumented-target/ cache key — so a bump can't drift between
install and cache. Bumping it should be a deliberate PR: install
the new version locally, regenerate baselines via `--update`, and
commit the JSON diff so reviewers can see whether the version
change shifted any per-file numbers.

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

The ratchet walks `crates/*/src/**/*.rs` and requires every file to be
EITHER in `files` (with a coverage baseline) OR in `excluded_files`
(with a one-line reason). Anything outside both is a new source file
the host forgot to register.

The current `excluded_files` entries fall into two shapes:

- **Static-only modules**: no executable lines for LCOV to instrument.
  Example: `crates/rubyrs/src/_cext_link_keep_alive.rs` (`#[used] static`
  declarations to defeat linker DCE on the cext ABI).
- **Feature-gated modules**: not compiled on the default CI build.
  Examples: `http_server.rs` (`_http_server` feature), `stdlib_vendor.rs`
  (`stdlib` feature), `vm/cext_wasi.rs` (`target_os = "wasi"`).

Adding a new source file to one of these shapes? Add it to
`excluded_files` with a brief reason. The PR diff shows the addition.

Other categories the script silently skips (no baseline entry needed):

- `crates/*/tests/**` — test files measure coverage OF library code,
  not OF themselves.
- `crates/*/examples/**`, `crates/*/benches/**`, `build.rs` — not part
  of the workspace coverage target.
- `crates/rubyrs/fuzz/**` — separate cargo package, not in the main
  coverage run.
- Third-party deps (`registry/`, git deps) — filtered to `crates/`
  only.

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
