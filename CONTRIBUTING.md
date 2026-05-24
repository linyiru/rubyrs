# Contributing to rubyrs

Thanks for taking a look. This document covers the practical PR flow.
For *why* the project is the way it is, read [docs/](docs/) — especially
[ARCHITECTURE.md](docs/ARCHITECTURE.md) and the
[ADRs](docs/adr/).

## Before you start

Check [docs/SUBSET.md](docs/SUBSET.md) and
[docs/ROADMAP.md](docs/ROADMAP.md) to see if your idea is in scope.
rubyrs is **not** trying to be a full Ruby. Some changes (running Rails,
implementing a JIT, supporting C extensions) are explicit non-goals.

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
3. **Add a fixture test** in `tests/fixtures/`:
   ```bash
   echo '...your Ruby program...' > tests/fixtures/myfeature.rb
   UPDATE_EXPECTED=1 cargo test --release myfeature
   # Inspect the generated .expected file.
   ```
   Register the test in `tests/integration.rs`.
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
- One file, `src/main.rs`. Don't split eagerly; see
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

A PR is mergeable when:

- `cargo test --release` passes locally and in CI.
- Fixtures match CRuby behaviour, or a deliberate divergence is
  documented (in code comment + CHANGELOG, ideally an ADR).
- No new warnings under `-D warnings`.
- CHANGELOG.md updated.

What doesn't gate a PR: rustfmt output, clippy warnings (we don't
enforce these in CI today), benchmark numbers (we eyeball, we don't
gate).

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
