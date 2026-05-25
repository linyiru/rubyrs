# 0012: Thin LTO in the release profile

## Status

Accepted (2026-05). `[profile.release] lto = "thin"` set in
the workspace `Cargo.toml`. Release-build wall-time costs
~3 s; recovers ~7% on the fizzbuzz 1M microbench after the
[vm/ split](0011-cruby-mirrored-vm-split.md). Dev and test
builds (which default to `lto = false`) are unaffected.

## Context

[ADR 0011](0011-cruby-mirrored-vm-split.md) split the
single-file `vm.rs` (6593 lines) into 17 per-type submodules
under `vm/`. The split was move-only — every test stayed
byte-identical to CRuby — and our public CHANGELOG entry
initially claimed "No perf regression; fizzbuzz 1M and
Counter.inc 1M benchmarks unchanged within noise."

That claim was a self-deception. It had been written from
memory, not measurement. When we actually ran an A/B against
the pre-split commit (`08f960d`) with `hyperfine --warmup 3
--runs 10` we saw:

| Build | Mean wall | σ |
|---|---|---|
| Pre-refactor (`08f960d`) | 349.5 ms | 5.6 ms |
| Post-refactor (no LTO) | 372.8 ms | 10.9 ms |

A 23 ms (~7%) slowdown, well outside σ. The cause: cross-
module call edges between `Vm::step`,
`Vm::lookup_method_cached`, `Vm::do_call`, the
`primitive_call` free fn, etc., couldn't inline at
`-C opt-level=3` alone the way they did when everything
lived in one compilation unit. With each module compiled
independently and the linker stitching them, the hot
dispatch loop now had real function-call cost across each
of the major call seams.

Three responses were on the table:

1. **Accept the regression.** 7% is small in absolute terms;
   the niche we serve (1.8 ms end-to-end Brewfile DSL hosting)
   would still beat CRuby by ~40×.
2. **Roll the split back.** Re-merge the submodules into one
   file. Reverses ADR 0011 and gives up the navigation /
   review / panic-budget wins.
3. **Enable LTO.** Re-establish cross-module inlining at link
   time. Trades release build-time for code quality.

## Decision

Option 3 with `lto = "thin"` (not full LTO).

```toml
# Cargo.toml
[profile.release]
lto = "thin"
```

A subsequent verification with thin LTO:

| Build | Mean wall | σ |
|---|---|---|
| Pre-refactor (no LTO) | 349.5 ms | 5.6 ms |
| Post-refactor (no LTO) | 372.8 ms | 10.9 ms |
| **Post-refactor + thin LTO** | **350.2 ms** | 6.9 ms |

Recovered to within noise of the pre-split baseline.

### Thin vs full LTO

We picked thin over full for build-time reasons:

| | Thin LTO | Full (`lto = true`) |
|---|---|---|
| Release build extra time | +3 s | +15 s |
| Code-size impact | minimal | smaller binary |
| Inlining across crate boundaries | yes (most cases) | yes (everything) |
| Parallelism | per-module units | single thread for the LTO pass |

The 12-second difference between thin and full was the
deciding factor. Our CI's perf-budget job rebuilds the
release binary every push; full LTO would have noticeably
slowed down that wall. Thin LTO closes 99% of the inlining
gap (post-refactor mean within 1 ms of pre-refactor) for
20% of the cost.

If a future workload turns up a hot path that thin LTO
still can't inline, the upgrade to `lto = true` is a one-
line change with an established build-time cost.

## Why not the other options

### Accepting the regression

7% sounds small; cumulatively it isn't. The same "we'll
accept this one" reasoning applied N times costs N% over
years of feature work — exactly the chain-degradation the
perf budget was set up to prevent (see `perf/README.md`).
The whole point of having ratchet-down baselines is
that each individual decision answers "is this regression
necessary?" — and here the answer was no.

It would also have made the CHANGELOG claim retroactively
honest only by lowering the bar. Worse outcome than fixing
the regression.

### Rolling the split back

ADR 0011 explicitly tested the structural-fix-for-perf-cost
trade. The navigation / review / panic-budget wins from
the split are durable; the inlining loss is purely a
codegen artifact that has a known cheap fix. Rolling
back would have spent the structural win to dodge a
configuration option.

### Profile-guided optimisation (PGO) instead of LTO

PGO could in principle do better than LTO by re-ordering
basic blocks on hot paths. We considered and deferred it
for three reasons:

1. PGO needs a profiling workload; rubyrs's "hot path"
   depends on which Ruby script the embedder runs. The
   training script becomes a knob nobody wants to tune.
2. PGO doubles build infrastructure complexity (two-stage
   build with an instrumented intermediate).
3. The 7% recovery we needed was structural (inlining),
   not micro (block ordering). LTO addresses the root
   cause; PGO would have papered over it.

## Trade-offs

### Cost: release build time + ~3 s

Local: M-series Mac, `cargo build --release -p rubyrs`
goes from ~6 s to ~9 s after the LTO step lands. CI
ubuntu-latest sees a similar absolute delta (perf-budget
job rebuild went from ~12 s to ~15 s).

We deemed this acceptable. The cargo release-build time
isn't on any developer's hot loop — dev work happens at
`cargo check` / `cargo test` time, both of which run on
the dev profile (no LTO).

### Cost: thin LTO doesn't always inline

A small number of cross-module call sites remain non-inlined
even under thin LTO (the linker decides per-edge, and very
cold paths can be skipped). If a future hot path shows up
that thin can't reach, the escalation is `lto = true`
documented above.

### Cost: this is a release-profile setting, not a code change

A consumer using rubyrs as a library inherits the release
profile of *their* binary, not ours. We rely on `cargo`'s
workspace-vs-consumer profile resolution rules, which mean
a downstream crate's release profile shadow ours. They
have to opt in to LTO themselves; we mention this in
`docs/DEVELOPMENT.md` and `BENCHMARKS.md` for embedders
who care about the canonical numbers.

### Benefit: structural-decision-perf-cost decoupling

The vm/ split decision in ADR 0011 stands on its own
merits (review, navigation, panic budget). LTO recovers
the codegen cost. The two decisions are independent:
either can be revisited later without binding the other.

### Benefit: future structural splits stay cheap

When we later moved cext-reentrance machinery into
vm/cext.rs (commit `1ad96df`), the move was a wash
performance-wise — thin LTO absorbed the additional
boundary. Without LTO already in place we would have
needed to budget perf headroom for every subsequent
structural change.

## Verification

The `crates/rubyrs/benches/fizzbuzz_1m.rb` benchmark is
checked in and CI-gated (see [ADR 0011 trade-offs]
above and `perf/baselines.tsv`). A regression in either
the codegen (LTO drift) or the dispatch loop (real
algorithmic regression) would land red on the CI's
perf-budget job — not silently in some future
hyperfine session.

## Related

- [ADR 0011 — CRuby-mirrored vm.rs split](0011-cruby-mirrored-vm-split.md)
- [`crates/rubyrs/benches/fizzbuzz_1m.rb`](../../crates/rubyrs/benches/fizzbuzz_1m.rb)
- [`perf/baselines.tsv`](../../perf/baselines.tsv)
- [`docs/BENCHMARKS.md`](../BENCHMARKS.md) — public-facing
  numbers; the "Reproducing" section now notes the LTO
  setting.
- [`CHANGELOG.md`](../../CHANGELOG.md) "Changed: lto = thin"
  entry (which also retired the earlier overstated claim).
