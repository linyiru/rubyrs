# 0021: OutputSink trait — no_std-compatible stdout abstraction

## Status

Accepted (2026-05-27). Implemented as a precursor to ADR 0018
Phase 1 (`rubyrs-core` extraction) — the trait is shipping
in the current single-crate layout so Phase 1's mechanical
migration doesn't need to also design + introduce it.

## Context

`Vm::stdout` is `Box<dyn std::io::Write>` today. The public
embed API is `Runtime::set_stdout(Box<dyn std::io::Write>)`.
Both depend on `std::io::Write`, which doesn't exist in
`no_std`.

ADR 0018 Phase 1 adds `#![no_std]` to a future `rubyrs-core`
crate. Without an abstraction, that PR has to either:

1. Define an output trait inline as part of the extraction
   (designing, debating, and migrating all at once — high
   risk for what's already a high-risk PR)
2. Pull a no_std-compatible IO crate (`embedded-io`,
   `genio`, etc.) as a dependency — adds ~1500 LOC of trait
   machinery for our needs (~5 method bodies)

[STD_AUDIT.md](../STD_AUDIT.md) Open Question #1 named this
as the design decision Phase 1 depends on. This ADR locks
the decision and ships the abstraction NOW so Phase 1 only
has to `git mv` it.

## Decision

Vendor a minimal `OutputSink` trait in
`crates/rubyrs/src/output.rs`. Public API:

```rust
pub trait OutputSink {
    fn write_bytes(&mut self, buf: &[u8]) -> Result<(), OutputError>;
    fn flush(&mut self) -> Result<(), OutputError> { Ok(()) }
    fn write_fmt(&mut self, args: fmt::Arguments<'_>) -> Result<(), OutputError> { /* default */ }
}

pub struct OutputError { /* owned String message */ }
pub struct NullSink;          // discards bytes
pub fn null_sink() -> Box<dyn OutputSink>;

#[cfg(feature = "std-sink")]
pub struct StdSink<W: std::io::Write>(pub W);
```

### Design rules

1. **`no_std + alloc` compatible.** The trait uses only
   `core::*` types in its signatures (`fmt::Arguments`,
   `Result`); allocations go through `alloc::*`
   (`Box`, `String` for `OutputError`'s message field).
   `extern crate alloc;` was added to `lib.rs` so this
   path works in today's `std` build and continues to
   work after Phase 1's `#![no_std]` migration.

2. **`write_fmt` provided by default.** The `write!` /
   `writeln!` macros expand to `.write_fmt(format_args!(…))`.
   To make those macros work on `Box<dyn OutputSink>`, the
   trait must have a `write_fmt` method. We provide it as
   a default that funnels into `write_bytes` via a tiny
   `core::fmt::Write` adapter — implementors only override
   `write_bytes` (+ optionally `flush`).

3. **No `Send + Sync` bound.** `Vm` is single-threaded.
   The boxed sink stored in `Vm::stdout` doesn't cross
   thread boundaries; bounding the trait would force
   embedders' custom sinks to be `Send + Sync` for no
   reason.

4. **Errors return a vendored `OutputError`.** We can't
   use `std::io::Error` (no_std). We could return
   `core::fmt::Error` for `write_fmt` and `()` for
   `write_bytes`, but losing the error message hurts
   debugging. `OutputError { msg: alloc::String }` is the
   minimum that captures the underlying cause.

5. **`StdSink` lives behind a feature flag.** A new
   `std-sink` Cargo feature (default-on today) gates the
   `StdSink<W: std::io::Write>` adapter. When ADR 0018
   Phase 1 lands, `rubyrs-core` will NOT have this
   feature; the facade `rubyrs` crate will. The
   `set_stdout(Box<dyn std::io::Write>)` public API stays
   in the facade.

6. **`NullSink` is the Tier 1 default.** ADR 0017 says
   the default stdout is "no sink — embedder must opt in
   via `set_stdout`". Today that's `std::io::sink()`.
   `NullSink` is the no_std equivalent: same semantics,
   no `std` dependency.

### What lives where (after Phase 1)

| Item | Crate / file (today) | Crate / file (post Phase 1) |
|---|---|---|
| `OutputSink` trait + `NullSink` + `OutputError` | `rubyrs/src/output.rs` | `rubyrs-core/src/output.rs` |
| `StdSink<W>` adapter | `rubyrs/src/output.rs` (under `#[cfg(feature = "std-sink")]`) | `rubyrs/src/output.rs` (facade crate; no feature gate needed once core is split) |
| `Runtime::set_stdout(Box<dyn std::io::Write>)` public API | `rubyrs/src/lib.rs` | `rubyrs/src/lib.rs` (facade) |
| `Vm::stdout` field | `Box<dyn std::io::Write>` (today) | `Box<dyn OutputSink>` (migrated in Phase 1) |

### Backward compatibility

The current public embed API
`Runtime::set_stdout(Box<dyn std::io::Write>)` is preserved
verbatim. Internally it will (post-migration) wrap the
input in `StdSink` before assigning to `Vm::stdout`.
Embedders do not see `OutputSink` unless they want to —
the canonical `Box::new(std::io::stdout())` /
`Box::new(Vec::<u8>::new())` patterns keep working.

A parallel `Runtime::set_sink(Box<dyn OutputSink>)` will
land at the time `Vm::stdout` migrates to OutputSink (Phase
1). Embedders who want the no_std-compatible path use the
new method; the old method stays for `std::io::Write`
ergonomics.

### Migration steps post-ratification

- **Today (this ADR + implementation)**: `output.rs` exists
  with the trait + adapter + 6 unit tests. `Vm::stdout`
  unchanged. No public API change.
- **Phase 1 (ADR 0018)**: `Vm::stdout` switches to
  `Box<dyn OutputSink>`. `output.rs` moves with
  `rubyrs-core`. The `set_stdout` API gets an internal
  wrap-in-`StdSink` step. `set_sink` added. `vm/kernel.rs`'s
  `write!` / `writeln!` call sites continue to work
  unchanged (auto-deref through Box → OutputSink::write_fmt).
- **Future**: deprecate `set_stdout` in favour of `set_sink`
  once a release has shipped with both. Not committed yet.

## Consequences

### What gets easier

- **Phase 1 PR is smaller.** The trait already exists +
  tested. Phase 1's job is `git mv output.rs
  crates/rubyrs-core/src/output.rs` + update `Vm::stdout`'s
  type. No design debate in Phase 1's PR.
- **`no_std` audit is partially complete.** One of the
  Tier 2-host-IO sites from STD_AUDIT.md (`vm.rs:410`'s
  `Box<dyn std::io::Write>`) has a known migration target.
- **Embedders who want a `no_std` sink can write one
  today.** A test fixture in `output.rs` itself
  demonstrates the pattern (CaptureSink).
- **`OutputError` is owned + propagates rich messages.**
  The today's `let _ = write!(vm.stdout, …)` pattern
  silently discards errors. Once Phase 1 migrates the
  call sites, errors propagate as Ruby `IOError`.

### What gets harder

- **Two methods to think about during the transition.**
  `set_stdout` (current) and `set_sink` (future). For
  one release cycle, both exist. The doc must call out
  that they're equivalent shapes.
- **Test fixtures change shape.** `vm/iter.rs:2574`'s
  `Sink` adapter (today: `impl std::io::Write`) will
  move to `impl OutputSink` in Phase 1. Tests using
  that adapter need a one-line change.
- **`write!`/`writeln!` macro behaviour relies on a
  trait method, not a foreign impl.** The macros work
  via method-call expansion; our `write_fmt` method must
  be inherent to the trait, not a foreign impl. The
  current design has it as a trait method with default
  body — works.

### What we explicitly accept trading away

- **A clean `core::fmt::Write` super-trait.** A
  `trait OutputSink: core::fmt::Write` would inherit
  `write_fmt` for free, but it would force every
  implementor to also implement `fmt::Write` (`&str`-only
  path). Awkward for `OutputSink` whose primary path is
  bytes. The default `write_fmt` impl in our trait does
  the equivalent work in ~15 lines.
- **`Read` symmetry.** No `InputSource` trait. Tier 1's
  embed API has no script-readable input today; if it
  ever does, we'll define `InputSource` mirroring this
  ADR's shape. Out of scope here.
- **`std::io::Result`-shape error type.** A simpler
  `Result<(), ()>` would drop the `OutputError`
  message field. Trade-off: debugging cost vs ~32 bytes
  per error. Most paths never produce an error; the
  bytes are paid only on failure.

## Alternatives considered

1. **Defer to Phase 1 — design inline.** Phase 1 PR
   designs `OutputSink`, migrates `Vm::stdout`, and
   extracts `rubyrs-core` in one shot. Rejected: too
   much in one PR; design decisions get bikeshed inside a
   mechanical-migration PR.

2. **Use `embedded-io` crate.** ~1500 LOC of trait
   machinery for `Read`, `Write`, `Seek` and async
   variants. Rejected: 30× our needs; pulls a transitive
   dep on `embedded-io-async` ecosystem we don't want.

3. **Use `genio`.** Older no_std IO trait crate, much
   smaller. Rejected: low maintenance activity (no
   commits in 2 years at time of writing); we'd be
   importing dead code.

4. **`trait OutputSink: core::fmt::Write`.** Inherit
   `write_fmt` for free. Rejected per "What we
   explicitly accept trading away" — forces every
   implementor to also implement the `&str`-only path.

5. **Bytes-only trait (no `write_fmt` method).** Force
   call sites to use `vm.stdout.write_bytes(s.as_bytes())`
   instead of `write!(vm.stdout, "{}", s)`. Cleaner trait
   but ~6 `write!` / `writeln!` call sites in
   `vm/kernel.rs` to rewrite. Rejected: those call sites
   are ergonomic as-is; the default `write_fmt` impl is
   one helper struct.

6. **Generic `set_stdout<S: OutputSink>(s: S)`.** Avoid
   the `Box`. Rejected: `Vm::stdout` needs type erasure
   to be storable across `Vm` constructions; generic
   ergonomics don't gain us anything without that
   storage.

## Related

- [ADR 0017 — Tier-1 boundary](0017-tier1-boundary.md)
  — line 47's "permitted host-side internal use of `std`"
  carves out the `set_stdout` mechanism. `OutputSink` is
  the no_std-compatible re-shape of that mechanism.
- [ADR 0018 — Workspace migration plan](0018-workspace-migration.md)
  — Phase 1's `rubyrs-core` extraction consumes this
  trait. STD_AUDIT.md Open Question #1 was the forcing
  function for this ADR.
- [ADR 0007 — Host embedding API](0007-host-embedding-api.md)
  — `Runtime::set_stdout`'s public API surface; this ADR
  is the internal-type story behind it.
- [STD_AUDIT.md](../STD_AUDIT.md) — flagged this design
  as the Phase 1 dependency.
