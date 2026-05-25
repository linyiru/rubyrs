# Testing strategy

> "A carpenter, on arriving at a site, builds the workstation for the day's
> project from the materials and tools at hand. We should be the same:
> while implementing rubyrs, build the tools that will guarantee its
> quality in the future."

This document explains how we keep rubyrs honest as it grows, and the
pipeline we are building to scale that.

## Four layers

```
┌─────────────────────────────────────────────────────────────────┐
│  Layer 1: unit tests (in modules, #[cfg(test)])                  │
│  - Tight Rust-level checks for individual functions              │
│  - Currently sparse; we lean on layers 2/3/4                     │
└─────────────────────────────────────────────────────────────────┘
┌─────────────────────────────────────────────────────────────────┐
│  Layer 2: integration fixtures (tests/integration.rs)            │
│  - .rb file + .expected golden file for stdout                   │
│  - tests/fixtures/errors/ for .expected_err (stderr) cases       │
│  - `cargo test` execs the rubyrs binary, diffs                   │
│  - UPDATE_EXPECTED=1 cargo test regenerates the goldens          │
└─────────────────────────────────────────────────────────────────┘
┌─────────────────────────────────────────────────────────────────┐
│  Layer 3: public API smoke tests (tests/embed.rs)                │
│  - Calls Runtime/Config/register_fn/set_stdout/format_trap       │
│  - Pins the embedding API surface so accidental breakage shows   │
│    up in CI; also exercises resource caps (fuel/heap/frames)     │
└─────────────────────────────────────────────────────────────────┘
┌─────────────────────────────────────────────────────────────────┐
│  Layer 4: ruby/spec coverage (planned, see below)                │
│  - Spec files from upstream ruby/spec, mechanically translated   │
│  - Run on rubyrs AND CRuby; compare PASS counts                  │
│  - SPEC_STATUS.md is auto-generated; coverage drives credibility │
└─────────────────────────────────────────────────────────────────┘
```

Layers 2 and 3 are *hand-curated* — we choose the cases. Layer 4 is
the antidote: an external standard chooses the cases for us.

## Why ruby/spec, not our own tests

[`ruby/spec`](https://github.com/ruby/spec) is the de facto executable
standard for Ruby semantics. CRuby, JRuby, and TruffleRuby all run it.
Picking it as our oracle means:

- **No bias** — we don't get to handpick what we test.
- **Credibility** — "we pass X% of ruby/spec" is the only meaningful
  compatibility metric outside the CRuby team's blessing.
- **Forced discipline** — we can't skip the hard semantic edges.

## The ingestion pipeline

ruby/spec uses **mspec**, a custom test framework. Running mspec inside
rubyrs requires roughly all of Ruby (classes, modules, Symbol,
interpolation, exception hierarchy, method_missing, file I/O, ...) —
that's the TruffleRuby/JRuby long-tail path, not a near-term option.

Instead, we treat ruby/spec as a **corpus** and ingest it mechanically:

```
ruby/spec  (git submodule)
   │
   ▼  ❶ tools/spec_extract  (Rust; uses our own ruby-prism binding)
   │   Parses spec files. For each `it` block, emits a self-contained
   │   .rb that asserts the same behaviour using primitive Ruby that
   │   rubyrs can already run.
   │
   ▼  tests/spec/core/integer/plus_0.rb, plus_1.rb, ...
   │
   ▼  ❷ tests/spec_diff.rs  (Rust integration test)
   │   For each generated .rb: run on rubyrs, compare PASS/FAIL count
   │   to CRuby. Test passes iff parity.
   │
   ▼  ❸ SPEC_STATUS.md  (auto-generated)
       Per-directory PASS rates. Commits land or don't based on what
       this report shows.
```

Each piece is a small, focused tool. The pipeline grows by teaching
`spec_extract` new patterns, not by re-doing manual work.

## Pipeline versions

`spec_extract` will support more mspec patterns over time. We treat each
pattern as a version increment:

| Version | Recognises | Estimated reach |
|---------|------------|----------------|
| v0.1 | `(expr).should == literal`, `expr.should == literal` | ~10–20% of spec files |
| v0.2 | `.should be_xxx` predicates, `raise_error(...)` | +10% |
| v0.3 | `-> { ... }`, `before / after` hooks | +10% |
| v0.4 | `shared_examples`, `include / extend` | +20% |
| ... | ... approaching mspec full | ... |

Anything it doesn't recognise is **skipped and logged** to SPEC_STATUS.md.
Progress is therefore always measurable and never blocking.

## Current state — manual translation baseline (2026-05)

Before the extractor exists, we maintain a small **manually-
translated baseline** under
[`crates/rubyrs/spec/ruby/`](../crates/rubyrs/spec/ruby/) that
mirrors a subset of upstream ruby/spec using the conventions
documented in
[`crates/rubyrs/spec/README.md`](../crates/rubyrs/spec/README.md).
The translation is deliberately mechanical (a fixed table of
`expr.should == val` → `assert_eq(expr, val)` rewrites) so the
v0.1 extractor can be validated against the same files later —
"does the tool produce the same output as a human did?" is the
useful first-pass acceptance criterion.

| Area | Files | Examples | Pass rate |
|---|---|---|---|
| Metaprog (ADR 0010 PoC) | 6 | 30 | 100% |
| `core/string` subset (sub, gsub, reverse, include, empty) | 5 | 35 | 100% |
| `core/method` subset (call, compose, curry, ==, to_proc, owner, receiver) | 7 | 37 | 100% |
| `core/unboundmethod` subset (==) | 1 | 6 | 100% |
| **Total** | **19** | **108** | **100%** |

Every example must pass — there is no "tagged divergent" lane
yet. Skipped upstream `it` blocks are noted in the spec file's
top-of-file comment with the reason (out of subset / out of
master / fixtures-dependent). When master lands a feature that
unblocks a previously-skipped block, the comment becomes the
ratchet: un-skip and re-test.

Progress beyond this baseline goes either by hand (one PR per
upstream area) until the v0.1 extractor lands, or by
extractor-then-curate once it does.

## Workflow for adding a feature

The intended pull request flow:

1. Pick a feature from [ROADMAP.md](ROADMAP.md), or a spec
   directory that is partly red.
2. Implement the language feature.
3. Write at least one hand-crafted fixture in `tests/fixtures/` to lock
   in observable behaviour. This stays small and human-readable.
4. Run `tools/spec_extract` to regenerate `tests/spec/`.
5. Run `cargo test`. New tests should pass; old ones must still pass.
6. Update SPEC_STATUS.md (or let CI do it).
7. Commit. Reference the spec coverage delta in the commit body.

This loop means every new feature is *automatically* graded against ruby/spec,
without anyone manually picking what to test.

## Why we are not running mspec inside rubyrs (yet)

Goal of mspec-inside-rubyrs is on the long-term [ROADMAP.md](ROADMAP.md)
under "Run mspec inside rubyrs". It only becomes meaningful once we have
~95% of mspec's own dependencies running — Symbol, interpolation, Module,
exception classes, method_missing, IO, Comparable, Enumerable as a mixin.
We get there by following the SPEC_STATUS.md report; once it crosses a
threshold the switch is mechanical.

Until then, the extractor approach gives us:
- Faster iteration (no Ruby host bootstrap)
- Identical CRuby/rubyrs comparison, byte-level
- A real number to report, every commit
