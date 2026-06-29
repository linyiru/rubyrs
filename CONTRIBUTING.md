# Contributing to rubyrs

Thanks for taking a look. This document covers the practical PR flow.
For *why* the project is the way it is, read [docs/](docs/) — especially
[ARCHITECTURE.md](docs/ARCHITECTURE.md) and the
[ADRs](docs/adr/).

## Before you start

Check [docs/SUBSET.md](docs/SUBSET.md) and
[docs/ROADMAP.md](docs/ROADMAP.md) to see if your idea is in scope.
rubyrs is **not** trying to be a full Ruby, but the scope has grown well
beyond the original subset. A native JIT (ADRs
[0030](docs/adr/0030-jit-tier.md) / [0032](docs/adr/0032-jit-native-surpass.md) /
[0034](docs/adr/0034-jit-first-surpass-yjit.md)), the C-extension ABI (the
`cext` *default* feature), and the blessed-gem compatibility menu (ADR
[0026](docs/adr/0026-omakase-blessed-gem-menu.md), targeting Rack / Sinatra /
ActiveSupport-class gems) are all active areas — not non-goals. Genuinely
out of scope: `Thread` / `Ractor` parallelism, an AOT compiler, arbitrary
`bundle install` from rubygems.org, and runtime `eval` of arbitrary strings
on the WASM / embed path.

For anything bigger than a small bug fix or a missing built-in, open an
issue first to align on scope.

## Setup

See [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) for build prerequisites
and the WASM target.

Quick start:

```bash
cargo build --release
cargo test --release
```

## Workflow

1. **Branch** from `master`.
2. **Implement** the change. Single concept per branch / PR.
3. **Add a fixture test** in `crates/rubyrs/tests/fixtures/`:
   ```bash
   echo '...your Ruby program...' > crates/rubyrs/tests/fixtures/myfeature.rb
   UPDATE_EXPECTED=1 cargo test --release -p rubyrs myfeature
   # Inspect the generated .expected file.
   ```
   Register the test in `crates/rubyrs/tests/integration.rs`.
4. **Run the full suite**: `cargo test --release`. All existing tests
   must still pass.
5. **Update CHANGELOG.md** under `[Unreleased]`. User-facing items go in
   `### Added` / `### Changed` / `### Fixed`. Internal-only items go in
   `### Internal`.
6. **Consider an ADR** if your change involves a non-obvious design
   choice. See [docs/adr/README.md](docs/adr/README.md).

## Coding style

- The codebase uses a deliberately compact style for short matches and
  test functions. `cargo fmt` is **not enforced** in CI. Match the
  surrounding code.
- rubyrs lives under `crates/rubyrs/`. Source files are kept
  small and focused, but resist eager splitting; see
  [ARCHITECTURE.md § Why a single file](docs/ARCHITECTURE.md#why-a-single-file).
- Comments explain *why*, not *what*. The code already says what.
- No new dependencies without discussion in an issue. The whole point of
  a Rust-based runtime is a small dependency closure.

## Commit messages

Single line summary (≤72 chars), then a blank line, then the body.
Don't include "Co-Authored-By:" trailers.

Bias the body toward **why**:

```
Add String#chomp

CRuby's chomp matches "\n", "\r", and "\r\n" trailing — our parser
already produces \n-only literals (no \r\n), so we only need the \n
case. If we later support \r\n input we'll revisit.

ruby/spec coverage delta: spec/core/string/chomp: 0/7 → 4/7.
```

## What gets merged

A PR is mergeable when **all** of the following hold. Green CI is
necessary but not sufficient — the non-CI items below count too.

Treat any red check as blocking, including on the master baseline —
fix master first, then merge. The compounded cleanup PRs in May 2026
(#36 / #39 / #41 / #45) were all "previous PR landed with one of
these red and we didn't notice until the next person's PR inherited
the failure".

The CI gates (every job in
[`.github/workflows/ci.yml`](.github/workflows/ci.yml) AND
[`.github/workflows/cargo-deny.yml`](.github/workflows/cargo-deny.yml)
must be green; the jobs themselves run in parallel — only steps
within each job have a fire order). For the
**what's-in-the-gate / how-to-run-locally / how-to-bump
reference**, see [`docs/CI_GATES.md`](docs/CI_GATES.md). The
list below names what blocks a merge:

- **`cargo clippy --release --all-targets --workspace -- -D warnings`**
  (Test job, both ubuntu + macos). Catches style / pedantic /
  soundness lints. The workspace is at zero warnings; this is the
  wall that keeps it there.
- **`cargo build --release`** with `RUSTFLAGS: "-D warnings"`.
- **`cargo test --release`** — full workspace test, diff_cruby
  suite included.
- **`cargo test --release` with `STRESS_GC=1`** — same suite under
  every-alloc GC. GC root holes silently corrupt slot state under
  normal collection; STRESS_GC turns them into reproducible test
  failures. Any PR adding a `heap.alloc` / `maybe_gc` site MUST run
  this locally before pushing.
- **Verify cext example bundles build** — independent
  `bash build.sh` runs so a path/flag regression in cext examples
  shows up here even if their integration tests were skipped.
- **Build (wasm32-wasip1)** — separate job. Doesn't run tests yet
  (no wasmtime smoke), only proves the crate still compiles for
  the WASI target.
- **Panic budget (per-file ratchet)** — separate job, see
  [`docs/PANIC_AUDIT.md`](docs/PANIC_AUDIT.md). Per-file counts of
  `panic!` / `.unwrap()` / `.expect()` are ratcheted: **the direction
  is always down, never up**. The intended workflow is to convert a
  panic site to a `Trap`, then lower the budget. Bumping a budget
  *up* is a documented escape hatch that needs reviewer agreement
  the new site is genuinely ICE-class (an invariant the compiler /
  dispatch loop already enforces) AND that no Trap-class alternative
  exists; the bump and the new site land in the same commit so the
  history shows the trade-off.
- **Perf budget (peak-RSS ratchet)** — separate job. Wall-time +
  peak-RSS for fixed inputs. Regressions surface as a hard fail.
- **Miri smoke (Stacked + Tree Borrows)** — separate job. Catches
  UB in the cext FFI surface (CURRENT_VM_PTR aliasing, etc.).
- **Supply-chain (cargo-deny)** — separate workflow file
  ([`.github/workflows/cargo-deny.yml`](.github/workflows/cargo-deny.yml)).
  Advisories / licenses / banned crates / source-registry pinning,
  per `deny.toml`. Path-filtered so docs-only / Ruby-source-only
  PRs skip it; weekly Sunday cron catches advisory-DB updates
  against a frozen Cargo.lock.

Also required for a merge:

- Fixtures match CRuby behaviour, or a deliberate divergence is
  documented (in code comment + CHANGELOG, ideally an ADR).
- CHANGELOG.md updated.

What doesn't gate: rustfmt output (we don't run it), benchmark
numbers beyond the perf-budget ratchet (we eyeball, we don't gate).

## Tests we particularly want

Things that move us toward "real Ruby" but are also tractable:

- ruby/spec coverage in `tests/spec/` once `tools/spec_extract` lands
- Edge cases for existing built-ins (negative indices, empty arrays,
  numeric overflow, etc.)
- WASM-build regression tests

Things we don't want yet:

- Performance micro-optimizations that complicate the dispatch loop
  without a clear benchmark win
- New built-ins copy-pasted from CRuby's stdlib without spec coverage
