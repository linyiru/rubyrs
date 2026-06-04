# CI gates

Map of every CI gate in the rubyrs workspace: what it checks,
how to inspect a failure locally, and where its source-of-truth
lives.

For the "is my PR ready to merge?" view, see
[`CONTRIBUTING.md`](../CONTRIBUTING.md#what-gets-merged). For
deep-dives on individual gates, the per-gate docs (linked
below) carry the design and bump etiquette. This file is the
**index** — the place to look when you need to know what's in
the picture or how to add a new gate.

## Overview

9 gates split across 5 workflow files. Two composite actions
factor common install logic so a toolchain or cargo-tool bump is
a one-line edit.

```
                      .github/actions/
                      ├── install-pinned-cargo-tool/   (cache + version-pinned cargo subcommand)
                      └── setup-rust-toolchain/        (reads rust-toolchain.toml)
                              ↑
                              │ used by 11 jobs across 5 workflow files
                              │
.github/workflows/
├── ci.yml              8 jobs:
│                        - Test (×2 OS: ubuntu, macos)
│                        - Build (wasm32-wasip1)
│                        - Panic budget (per-file ratchet)
│                        - Perf budget (peak-RSS ratchet)
│                        - Perf budget (JSON bench per-iter)
│                        - Miri smoke (Stacked + Tree Borrows)
│                        - Coverage (per-file ratchet)
│                        - Framework parity (real Sinatra/Rack gems)
├── cargo-deny.yml      Supply-chain (cargo-deny) — paths-filtered + weekly cron
├── gapscan-pr.yml      Per-PR subset-coverage diff comment (advisory, not a gate)
├── fuzz.yml            libfuzzer-sys nightly (scheduled, not a per-PR gate)
└── wasm-breakdown.yml  Cold-start measurement (scheduled, not a gate)
```

Required-status-check name preserved: cargo-deny.yml uses
`name: CI` so its check label is `CI / Supply-chain (cargo-deny)`
— byte-identical to its pre-extraction form, no branch-
protection cutover needed.

## The gates

### Test — `ci.yml` job `test`

Matrix on `[ubuntu-latest, macos-latest]`. Builds the workspace
under `RUSTFLAGS=-D warnings`, runs `cargo test`, then re-runs
under `STRESS_GC=1` (every-allocation GC — turns GC root holes
into reproducible failures). Also: clippy at zero warnings, two
structural greps (GC rooting lint, rustdoc orientation lint), and
a belt-and-suspenders cext-bundle build verification step.

- **Local**: `RUSTFLAGS="-D warnings" cargo test --release`
  (CI sets `RUSTFLAGS=-D warnings` globally, so a bare
  `cargo test --release` can pass locally while CI fails on a
  warning-as-error). Add `STRESS_GC=1` before pushing if your PR
  touches `heap.alloc` / `maybe_gc` call sites.
- **Bump policy**: clippy lint hits → fix or
  `#[allow(clippy::xxx)]` with a rationale comment.
- **See also**: [`CONTRIBUTING.md`](../CONTRIBUTING.md#what-gets-merged)
  for the complete merge-gate checklist.

### Build (wasm32-wasip1) — `ci.yml` job `wasm`

Builds the crate for wasm32-wasip1 with `--no-default-features`
(cext requires `libloading`/`dlopen` which wasm32-wasi doesn't
have — see [ADR 0015](adr/0015-wasm32-wasip1-cext.md)). Then
`perf/wasm_check.sh` does the AOT-compile + wizer pre-init +
cold-start measurement.

- **Local**: `cargo build --target wasm32-wasip1 --no-default-features`
  (or `bash perf/wasm_check.sh` for the full pipeline).
- **Bump policy**: wasi-sdk 24 pin in ci.yml + `WASI_SDK_PATH`
  env. Bumping needs a re-run of the wasm-breakdown.yml workflow
  to confirm cold-start hasn't regressed.

### Panic budget — `ci.yml` job `panic-budget`

Per-file count of `panic!` / `.unwrap()` / `.expect()`,
ratcheted: **direction is always down, never up**. The intended
workflow is to convert a panic site to a `Trap`, then lower the
budget.

- **Local**: `bash scripts/panic-budget.sh` (greps + diffs
  against in-tree per-file JSON budgets).
- **Source of truth**:
  `crates/rubyrs/data/panic_budgets/*.json`.
- **Bump policy**: lowering = good (lands with the conversion
  commit). Raising = needs reviewer agreement the new site is
  ICE-class (invariant the compiler/dispatch loop already
  enforces) AND no Trap-class alternative exists; the bump and
  the new site land in the same commit so the history shows the
  trade-off.
- **See also**: [`docs/PANIC_AUDIT.md`](PANIC_AUDIT.md).

### Perf budget (peak-RSS ratchet) — `ci.yml` job `perf-budget`

Wall-time + peak-RSS for fixed-input scripts. Regressions fail
the gate hard. Same ratchet philosophy as panic-budget.

- **Local**: `bash perf/check.sh`.
- **Source of truth**: [`perf/baselines.tsv`](../perf/baselines.tsv).
- **Bump policy**: lowering = lands with the optimization PR
  (the diff describes the win). Raising = needs a documented
  reason in the PR (e.g., a feature-flag-gated dep with no
  cheaper alternative).

### Perf budget (JSON bench per-iter) — `ci.yml` job `json-perf-budget`

Same shape as perf-budget but for the `_json_native` feature's
serde_json accelerator. Per-iteration walltime budgets prevent
the JSON path from regressing against CRuby's stdlib.

- **Local**: `bash bench/json_bench_check.sh` (the exact command
  CI's `json-perf-budget` job runs).
- **Source of truth**: `bench/json_bench_baselines.tsv`.
- **Bump policy**: same as perf-budget.

### Miri smoke — `ci.yml` job `miri`

Runs unit tests under miri's Stacked Borrows + Tree Borrows
aliasing models. Catches UB in the cext FFI surface
(`CURRENT_VM_PTR` aliasing, etc.) that compiler-side
optimizations could later weaponize into miscompiles.

- **Local**: `cargo +nightly miri test` (slow — only run when
  touching unsafe code).
- **Toolchain**: nightly (via composite's `channel-override:
  nightly`). Date is unpinned by design — we want the freshest
  miri rules.
- **See also**: ADR 0013's Miri verification record.

### Coverage — `ci.yml` job `coverage`

Per-file line% ratcheted DOWN. cargo-llvm-cov generates LCOV,
`scripts/coverage_ratchet.py` compares against in-tree baselines.
Tolerance absorbs sub-1% noise from minor refactors; intentional
drops are reviewed via the baseline-JSON diff.

- **Local**: see [`docs/COVERAGE.md`](COVERAGE.md) for the full
  workflow. TL;DR:
  ```bash
  cargo install cargo-llvm-cov --locked --version 0.8.7
  cargo llvm-cov --workspace --all-targets --lcov --output-path lcov.info
  python3 scripts/coverage_ratchet.py --lcov lcov.info \
      --baseline crates/rubyrs/coverage_baseline.json
  ```
- **Source of truth**:
  [`crates/rubyrs/coverage_baseline.json`](../crates/rubyrs/coverage_baseline.json).
- **Bump policy** (the cargo-llvm-cov version pin): edit
  `CARGO_LLVM_COV_VERSION` in `ci.yml`'s coverage job; bumping
  the tool may change instrumentation semantics, so regenerate
  baselines via `--update` and commit the JSON diff so reviewers
  can see whether the version change shifted any per-file
  numbers.

### Framework parity — `ci.yml` job `framework-parity`

Diff against real Ruby gems vendored 1:1 (sinatra_lite,
rack_cors, sinatra_cors, sinatra_param, etc.). Runs the gem's
public API surface through both rubyrs and the real
CRuby+gem stack; asserts byte-identical output. Catches DSL host
gaps that surface only against actual gem code, not synthetic
fixtures.

- **Local**: `cargo test --release --features
  default,stdlib,_http_server,_fiber,_json_native --test
  diff_framework -- --nocapture`.
- **Bump policy**: gem-version bumps land alongside the
  vendoring update. New parity fixtures are added in their own
  PR per ADR 0026 (Omakase menu).

### Supply-chain (cargo-deny) — `cargo-deny.yml`

Advisories (RustSec) + licenses + banned crates + source-
registry pinning. Standalone workflow (not folded into ci.yml)
because a transient advisory-DB flake shouldn't cascade into
the main Test job.

- **Local**: `cargo install cargo-deny --locked --version 0.19.8;
  cargo deny check`.
- **Source of truth**:
  [`deny.toml`](../deny.toml) (workspace root).
- **Trigger**: paths-filtered — only runs on PRs touching
  Cargo.lock, `**/Cargo.toml`, deny.toml, rust-toolchain.toml,
  or the workflow itself. Weekly Sunday 12:00 UTC cron catches
  advisory-DB updates against a frozen Cargo.lock.
- **Bump policy**: bumping the cargo-deny pin or adding a
  license/exception is a deliberate commit; the new ruleset must
  pass locally before push. Pin lives at the
  `install-pinned-cargo-tool` composite's `version:` input in
  `cargo-deny.yml`.

## Composite actions

### `install-pinned-cargo-tool` — for pinned cargo subcommands

Consumed by: `cargo-deny.yml`, `ci.yml`'s `coverage` job.

Inputs: `tool:` (crate AND binary name — 1:1 mapping assumed),
`version:` (semver). Builds a cache key from
`<runner.os>-cargo-tool-<tool>-<version>-<lockhash>`, performs
strict version-equality check against `~/.cargo/bin/<tool>` (not
PATH lookup, not the bare-`command -v` form), and reinstalls
with `--force` on mismatch. Post-install re-verify confirms the
just-installed binary reports the pinned version.

Cargo-subcommand-wrapper invocation handled via `${TOOL#cargo-}`
derivation, so cargo-llvm-cov / cargo-audit / cargo-vet /
cargo-semver-checks (all require `argv[1] == <subcommand>`) work
uniformly with cargo-deny (which is tolerant of either form).

### `setup-rust-toolchain` — reads `rust-toolchain.toml`

Consumed by: all 11 toolchain-needing jobs across all 5 workflow
files. Replaces `dtolnay/rust-toolchain@stable` (silent bypass)
and `dtolnay/rust-toolchain@master with toolchain: "X.Y.Z"`
(duplicated pin).

Inputs:
- `channel-override:` — for jobs that intentionally diverge from
  the workspace pin (miri uses `nightly`). When set, file
  components are ALSO ignored (components are channel-tied).
- `toolchain-file:` — defaults to workspace `rust-toolchain.toml`;
  fuzz overrides with `crates/rubyrs/fuzz/rust-toolchain.toml`
  (detached sub-workspace).
- `targets:` / `components:` — forwarded; file's components are
  merged with caller's so jobs get `rustfmt`+`clippy` from the
  workspace toml plus their task-specific extras.

## Source-of-truth files

Each pin / baseline lives in exactly one canonical location.
Bumping is a one-line edit in that file; CI auto-follows.

| File | What it pins |
|---|---|
| [`rust-toolchain.toml`](../rust-toolchain.toml) | Workspace Rust channel + base components |
| [`crates/rubyrs/fuzz/rust-toolchain.toml`](../crates/rubyrs/fuzz/rust-toolchain.toml) | Fuzz sub-workspace nightly date |
| [`deny.toml`](../deny.toml) | Supply-chain policy (advisories, licenses, bans, sources) |
| [`crates/rubyrs/coverage_baseline.json`](../crates/rubyrs/coverage_baseline.json) | Per-file coverage ratchet |
| [`perf/baselines.tsv`](../perf/baselines.tsv) | Peak-RSS + walltime budgets |
| `crates/rubyrs/data/panic_budgets/*.json` | Per-file panic counts |
| `cargo-deny.yml` composite's `with: version:` | cargo-deny version pin (literal copies in `deny.toml` header and `docs/DEVELOPMENT.md` are manual-sync) |
| `ci.yml` coverage job's `env: CARGO_LLVM_COV_VERSION` | cargo-llvm-cov version pin (literal copy in `docs/COVERAGE.md` is manual-sync) |

## Common workflows

### Bumping a workspace toolchain version

```diff
 # rust-toolchain.toml
 [toolchain]
-channel = "1.95"
+channel = "1.96"
 components = ["rustfmt", "clippy"]
```

That's it. 9 jobs auto-follow via the `setup-rust-toolchain`
composite. If 1.96 introduces new clippy lints, fix them in the
same PR (or add `#[allow(clippy::xxx)]` with rationale).

### Bumping a pinned cargo tool

For cargo-deny: edit `with: version:` in `cargo-deny.yml`. Re-run
`cargo deny check` locally before push to confirm the new ruleset
passes. Update the manual-sync literal in `deny.toml`'s header
and `docs/DEVELOPMENT.md` to match.

For cargo-llvm-cov: edit `CARGO_LLVM_COV_VERSION` in `ci.yml`'s
coverage job. Bumping may change instrumentation semantics —
regenerate `coverage_baseline.json` via `--update` and commit
the diff. Update the manual-sync literal in `docs/COVERAGE.md`.

### Lowering a budget after an optimization / panic-conversion

Land the code change and the budget edit in the **same commit**
so git blame shows the trade-off. Reviewers compare the JSON /
TSV diff against the PR's description of the win.

### Raising a budget (rare)

Same commit as the cause. PR description must explain why the
regression is acceptable (e.g., a new ICE-class panic with no
Trap-class alternative, a feature-flag-gated dep with no cheaper
shape). Treat as a deliberate exception, not a normal flow.

### Adding a new gate

Established conventions:

- **Standalone gate?** → its own workflow file (like
  `cargo-deny.yml`). Use this when the gate has a distinct
  flake risk profile, needs `paths:` filtering, or has its own
  cadence (cron).
- **Part of ci.yml?** → add a job. Use this when the gate runs
  on every PR with the same triggers as the rest.
- **Pinned cargo tool?** → use the
  `install-pinned-cargo-tool` composite. Same shape eliminates
  bare-`command -v` foot-gun and re-introduces nothing the
  existing callers' reviews already addressed.
- **Rust toolchain?** → use the `setup-rust-toolchain`
  composite. Don't open-code `dtolnay/rust-toolchain@stable`
  (silent pin bypass) or `@master with toolchain: "X.Y.Z"`
  (duplicates the pin).
- **Paths filter?** → see `gapscan-pr.yml` and `cargo-deny.yml`
  for the convention (workflow-level `paths:`, identical lists
  under both `push:` and `pull_request:`, weekly cron to catch
  drift against frozen state).
- **Required status check?** → set `name: CI` on the workflow
  (or accept a new check name in branch-protection).

### Debugging a failing gate

1. Find the gate in the table above. Note the local-run command.
2. Run it locally. If it reproduces, the CI failure is
   real-and-yours.
3. If it doesn't reproduce, check the cache. The cache keys use
   `hashFiles('Cargo.lock')`, which hashes file *contents*, not
   mtime — a noop `touch` won't change the key, so the stale
   cache still hits. Force a miss by actually changing
   `Cargo.lock` (e.g. `cargo update -p <crate>`) or by deleting
   the cache entry in the GitHub Actions UI.
4. If still doesn't reproduce, the runner image may have
   drifted (especially for `test` / `coverage` which depend on
   preinstalled toolchains). Check whether GHA's ubuntu-latest
   image rev changed recently.

## History

The supply-chain gate landed in PR #334 and was hardened across
PRs #339, #346, #352:

- **PR #334** — initial cargo-deny gate.
- **PR #339** — extracted to standalone workflow file with
  paths filter, `name: CI` to preserve check label.
- **PR #346** — `install-pinned-cargo-tool` composite (replaces
  bespoke install logic in cargo-deny.yml + coverage job; fixes
  cargo-llvm-cov's argv[1]-subcommand-wrapper invocation).
- **PR #352** — `setup-rust-toolchain` composite (reads
  `rust-toolchain.toml`, eliminates `@stable` silent bypass and
  hard-coded version duplication; honors file components).

Coverage / panic / perf gates pre-date this session — see
their respective dedicated docs for those histories.
