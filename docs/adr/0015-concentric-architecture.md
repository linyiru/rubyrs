# 0015: Concentric architecture via tiered Cargo features

## Status

Proposed (2026-05).

## Context

rubyrs today is positioned as an embeddable Ruby-subset interpreter —
the mruby niche. The README explicitly disclaims being a CRuby
replacement.

That framing is starting to bind. Two real pressures push outward:

1. **rubund** (Bundler-in-Rust) lives in this workspace and needs more
   Ruby surface than a strict "DSL host" subset (string IO, simple
   process control, more of the stdlib).
2. The longer-term question — "could this eventually run Sinatra? Rack?
   one day Rails?" — keeps coming up. Saying "no, never" closes a door
   that the architecture doesn't actually have to close.

We have to decide **before** stdlib expansion starts, because the wrong
shape locks in:

- Are we mruby-shaped (small, sandboxed, embeddable forever) or
  CRuby-shaped (full stdlib, C-ext ABI, ObjectSpace, eventually Rails)?
- Those are not "size on a slider". Many architectural decisions point
  in opposite directions (e.g. ObjectSpace introspection vs. tight
  object layout; capability-gated IO vs. POSIX direct; reject C-ext ABI
  vs. carry compatibility shims).

Picking either pole forecloses the other. We've seen what happens at
both ends:

- **Artichoke** tried to be both at once: workspace splits, `no_std`,
  Cargo features, backend abstraction — architecturally sound. It still
  stalled. Not because the architecture was wrong, but because trying
  to deliver all of Ruby (including C-ext ABI) with a single maintainer
  exhausts the budget before any layer ships.
- **mruby** held the small/embedded pole successfully for 15 years but
  is permanently boxed out of the Rails conversation.
- **wasmtime** is the proof that "one codebase, many shapes" works in
  Rust: a 5 MB embedded WASM runtime and a 100 MB+ full WASI host with
  Cranelift come out of the same `cargo build`, differing only in
  enabled features.

We don't have to pick a pole. We have to pick a **shape** that lets us
sequence outward without architectural rework.

## Decision

Adopt a **concentric architecture**: a tight inner core that's
shippable on its own, with strictly opt-in outer tiers added through
Cargo features.

### The tiers

```
┌─────────────────────────────────────────────────┐
│  Tier 4: mri-compat — CRuby ABI bet (v3+)       │  c-ext-abi, fiddle,
│  ┌───────────────────────────────────────────┐  │  object-space,
│  │  Tier 3: stdlib — full Ruby stdlib (v2)   │  │  marshal-compat
│  │  ┌─────────────────────────────────────┐  │  │
│  │  │  Tier 2: language — RubySpec ≥85%   │  │  │  net, openssl,
│  │  │  ┌───────────────────────────────┐  │  │  │  process, full IO,
│  │  │  │  Tier 1: core — today (v0.x)  │  │  │  │  ObjectSpace shim
│  │  │  │  parser, VM, GC, GC heap,     │  │  │  │
│  │  │  │  Block/Proc, basic classes,   │  │  │  │  fiber, ractor,
│  │  │  │  capability-gated IO, embed   │  │  │  │  thread, complete
│  │  │  │  API, sandbox, WASM target    │  │  │  │  metaprogramming
│  │  │  └───────────────────────────────┘  │  │  │
│  │  └─────────────────────────────────────┘  │  │
│  └───────────────────────────────────────────┘  │
└─────────────────────────────────────────────────┘
```

| Tier | Cargo feature | Target version | Use case |
|------|---------------|----------------|----------|
| 1 | `core` (default) | v0.x → v1.0 | Embed, DSL host, WASM, sandboxed scripting |
| 2 | `language` | v1.0 → v2.0 | Full Ruby semantics. Sinatra/Rack. |
| 3 | `stdlib` | v2.0 → v3.0 | Pure-Ruby gems. CLI tools. |
| 4 | `mri-compat` | v3.0+ (bet) | Real-gem compatibility. Eventually Rails. |

### Architectural rules

These are how we keep the inner core honest when outer tiers ship.

1. **The core is the constitution.** Every architectural decision is
   judged by "does this serve Tier 1?" If Tier 2/3/4 wants something
   that costs Tier 1 size, cold start, or sandbox guarantees, the core
   wins and the outer tier eats the cost.

2. **Outer tiers are opt-in, not opt-out.** Default Cargo features
   include `core` only. A user who runs `cargo install rubyrs` gets
   the embedded runtime, not the Rails-aspirational build. Activating
   higher tiers is an explicit choice.

3. **No outer-tier hooks in core code.** Core does not contain
   `#[cfg(feature = "object-space")]` shims, function-pointer holes
   "for future ABI compat", or empty stub structs reserved for stdlib.
   When a tier is off, the code does not exist.

4. **Workspace split mirrors tiers.** Each tier is its own crate in
   the workspace:

   ```
   crates/
     rubyrs-core/          # Tier 1 — depends on: prism, alloc
     rubyrs-language/      # Tier 2 — depends on: rubyrs-core
     rubyrs-stdlib/        # Tier 3 — depends on: rubyrs-language
     rubyrs-mri-compat/    # Tier 4 — depends on: rubyrs-stdlib
     rubyrs/               # CLI/facade — feature-gated re-export
   ```

   `rubyrs-core` must build standalone with `--no-default-features`.
   Verified in CI on every PR.

5. **`no_std` ratchet in `rubyrs-core`.** Core is `#![no_std]` +
   `extern crate alloc`. This forces every dependency on `std` to be
   an explicit, reviewed decision rather than ambient drift. This is
   what guarantees WASM, embedded, and sandbox targets stay viable.

6. **C-ext ABI stays out of v1 and v2.** Tier 4 is a public bet, not
   a covenant. The roadmap states explicitly: "C-ext ABI compatibility
   is a v3+ research direction. v1 and v2 ship Rust-native extension
   API only (`#[derive(Ruby)]`-style)."

7. **Outer tiers cannot pessimise inner-tier benchmarks.** Three CI
   metrics, gated per PR:
   - `core`-only binary size (currently ~4 MB, ceiling: 6 MB)
   - `core`-only cold start (currently ~1.5 ms, ceiling: 5 ms)
   - `core`-only embed RSS for `puts 1+2` (ceiling: 8 MB)

   A PR that lifts any ceiling has to either justify the lift or move
   the offending code to an outer tier.

### Cargo.toml shape

```toml
[features]
default = ["core"]

# Tiers
core = []
language = ["core", "_fiber", "_ractor", "_thread"]
stdlib = ["language", "_net", "_io-full", "_process", "_openssl"]
mri-compat = ["stdlib", "_object-space", "_marshal-compat", "_c-ext-abi"]
rails = ["mri-compat", "_fiber-scheduler"]

# Cross-cutting toggles (independent of tier)
sandbox = ["core"]      # capability-gated IO; required for untrusted scripts
wasm = ["core"]         # ensure no syscall paths slip in

# Internal building blocks — not for direct user use (underscore prefix
# is convention; see ADR 0007 for naming pattern in embed API)
_fiber = []
_ractor = []
_thread = []
_net = ["dep:hyper-light"]
_openssl = ["dep:rustls"]
# ...
```

### What this is not

- It is **not** a backend-pluggability story. We have one VM (see
  ADR 0002) and one parser (ADR 0001). The tiers are additive layers
  on a fixed core, not interchangeable engines. Artichoke's mruby ↔
  own-VM swap is exactly the kind of churn we are ruling out.
- It is **not** a perf tier system. All tiers share the same VM, GC,
  and dispatch. A `language`-tier build is not faster or slower at
  shared opcodes than a `core`-tier build; it just has more of them.
- It is **not** an excuse to defer hard choices. Each tier still has
  to ship. We commit publicly to "Tier 1 first, no Tier 2 work until
  the embedded story is genuinely good."

## Consequences

### What gets easier

- **One codebase, multiple deployment shapes.** Same git tree produces
  a 5 MB sandboxed WASM runtime and (eventually) a 100 MB Rails-trying
  desktop binary. Like wasmtime.
- **Honest marketing.** We can say "embeddable Ruby in Rust" today and
  "Rails-capable bet at v3+" tomorrow without lying or pivoting the
  repo. The README's "today / tomorrow" framing has architectural
  backing.
- **Sequenced delivery.** Each tier ships independently. Tier 1
  contributors don't get blocked on Tier 3 design debates. New
  contributors can pick a tier.
- **Bounded blast radius for hard problems.** C-ext ABI is the
  classic Rust-Ruby black hole. It now lives in one optional crate
  (`rubyrs-mri-compat`) and one feature flag. If it fails, the core
  is unaffected.
- **`rubund` has a clear target.** It depends on `language` or
  `stdlib`, not `core`. The "how much stdlib does Bundler need?"
  question becomes the de-facto driver of Tier 2/3 scope.

### What gets harder

- **More crates to navigate.** The workspace grows from ~2 crates to
  5-6. CI gets longer (each tier builds and tests independently).
  Cross-crate refactors need more touch points.
- **Discipline overhead.** Every PR has to ask "which tier does this
  belong to?" Reviewers have to enforce "no, this Tier 3 feature
  doesn't go in `rubyrs-core`". The temptation to bleed will be
  constant.
- **Some duplication.** Trait definitions in core that get implemented
  per-tier (e.g. `Io` trait with `CapabilityIo` core impl and
  `PosixIo` stdlib impl) cost more code than a single concrete impl
  would.
- **`no_std` is contagious in `rubyrs-core`.** Dependencies that
  assume `std` (e.g. `std::collections::HashMap`'s `RandomState`,
  `std::time`, env vars) can't be used without thought. Today
  `rubyrs` does use `std`; the migration is real work tracked
  separately.
- **Tier-4 expectations have to be actively managed.** Public roadmaps
  that say "Rails by v3" will be read as a promise. We have to use
  bet/research language consistently or get burned.

### What we explicitly accept trading away

- **Short-term simplicity.** A single-crate single-feature-set
  architecture would ship faster this quarter. We give that up for
  the optionality of multi-tier targets.
- **Bevy/tokio-level fine-grained feature matrix.** We're not going
  to ship 30 features. Four tiers + two cross-cutting toggles. We
  choose legibility over maximal modularity.
- **Backend pluggability.** No mruby fallback, no Truffle interop,
  no second VM. One core, layered outward.

## Alternatives considered

1. **Stay single-tier "embedded subset" forever.** The mruby pole.
   Honest and shippable, but closes the door on rubund's growing
   needs and the Rails conversation. The reason we're writing this
   ADR is that this is what we have today and it's starting to bind.

2. **Pivot to single-tier "full Ruby implementation".** The CRuby
   pole. Drops the embed/WASM story that already works. Bet
   everything on an unprovable claim. Artichoke tried this; one
   maintainer cannot deliver it.

3. **Two separate projects.** Fork the embed runtime as `rubyrs`
   and start `rubyrs-full` (or similar) as a separate repo. Doubles
   the maintenance, splits the brand, halves the contributors.
   Wasmtime did not do this and is healthier for it.

4. **Runtime modes instead of compile-time features.** A single
   binary that toggles `sandbox` / `full` / `mri-compat` at startup.
   Conceptually clean but ships unused code in every binary. Kills
   the "5 MB embed" pitch. Cargo features are the Rust-idiomatic
   answer to this question.

## Related

- [ADR 0001 — Prism as the parser](0001-prism-as-parser.md) — the
  fixed parser. Concentric tiers do not change the parser choice.
- [ADR 0002 — Bytecode VM, not a JIT](0002-bytecode-vm-not-jit.md) —
  the fixed VM. Concentric tiers do not change the VM.
- [ADR 0007 — Host embedding API](0007-host-embedding-api.md) — the
  embed API is core's primary product. Tier 1 maturity is gauged by
  this surface.
- [ADR 0008 — Resource caps for untrusted scripts](0008-resource-caps-for-untrusted-scripts.md)
  — the `sandbox` cross-cutting toggle subsumes this; resource caps
  remain in core and become a Tier 1 guarantee.
- [`docs/ROADMAP.md`](../ROADMAP.md) — public sequencing of tier
  delivery.
