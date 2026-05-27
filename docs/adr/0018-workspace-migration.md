# 0018: Workspace migration plan for the concentric architecture

## Status

Proposed (2026-05).

## Context

[ADR 0015](0015-concentric-architecture.md) committed rubyrs to a
four-tier concentric architecture with one crate per tier:

```
crates/
  rubyrs-core/          # Tier 1 — depends on: prism, alloc
  rubyrs-language/      # Tier 2 — depends on: rubyrs-core
  rubyrs-stdlib/        # Tier 3 — depends on: rubyrs-language
  rubyrs-mri-compat/    # Tier 4 — depends on: rubyrs-stdlib
  rubyrs/               # CLI/facade — feature-gated re-export
```

[ADR 0017](0017-tier1-boundary.md) settled *what* belongs inside
Tier 1. Two empirical PoCs verified the feature-gating mechanism
itself: the `cext` gate (PRs #73 + #75) and the `regex` gate
(PR #86), with binary-size savings measured at merge time and
recorded in those PRs' descriptions (single-digit-MB territory
on the smallest viable build; the absolute numbers shift with
toolchain versions, so they live in the PRs rather than here).

What neither ADR specifies: **how do we get from one
`rubyrs` crate to the four-crate split without freezing feature
work, breaking embed users, or letting `std` drift back into the
core during the migration?**

Today's reality:

- The workspace has 5 crates (`rubyrs`, `rubyrs-cext`,
  `rubund`, `rubyrs-gapscan`, `rubyrs-spec-extract`) — none of
  them match the tier shape ADR 0015 calls for.
- `crates/rubyrs/src/` is **~19 k lines** across `vm/*.rs` +
  top-level modules. There are **54 `use std::` sites**, none
  of them annotated for which tier they should live in.
- Feature work is *active* — gap reports have been moving 1–2
  percentage points per week against real codebases (see
  `docs/gap-reports/`), and multiple feature PRs are typically
  open at any given time. A long freeze would be expensive and
  would also lose the contributor-attention signal that says
  "this project is alive".
- The next architectural decision waiting on this — **BigInt /
  Integer-as-Bignum** — depends on which tier owns
  `num_bigint`. Integer is Tier-1 semantics; `num_bigint` is a
  Tier-1 implementation dependency. ADR 0015's "core wins on
  size vs semantics" rule (Rule 1) says BigInt lives in
  `rubyrs-core` behind a default-on `bignum` feature. But that
  decision is hypothetical until `rubyrs-core` *exists*.

Without an ordering ADR, the migration risks one of three
failure modes seen in similar projects:

1. **Big-bang split** — one giant PR that touches every file,
   blocks all feature work for weeks, and is too large to
   review. Artichoke's workspace-split PR was rejected for this
   shape.
2. **Drift split** — partial extraction that leaves `core`
   importing from outer tiers via re-exports or `pub(crate)`
   loopholes. Once that lands, the "no outer-tier hooks in
   core code" rule (ADR 0015 Rule 3) becomes unenforceable.
3. **Reverse split** — `core` ends up depending on the
   "convenience" outer layer via an extracted-but-unmoved
   helper. mruby avoided this for 15 years specifically by
   forbidding it in the build system; we need the same.

## Decision

Migrate in **six phases**, with each phase shippable as its own
PR (or short PR chain). No global freeze. Phase 1 alone has a
local "no new core surface" rule; everything else can land in
parallel.

### Phase 0 — preflight (this ADR + audit) — **before any code moves**

This ADR is Phase 0's first deliverable, landing on its own.
Phase 0's remaining deliverables land in **separate follow-up
PRs** so each can be reviewed against its own subject matter
(an ADR change shouldn't be reviewed in the same diff as a 54-
site `std` inventory):

- A **`std`-usage audit** in a new `docs/STD_AUDIT.md` (separate
  from `docs/MUTABLE_LAYERS.md`, which is specifically about
  interior-mutability layers in the metaprogramming runtime —
  mixing the two topics would make both docs harder to find
  later). The audit lists every one of the 54 `use std::` sites
  and tags each:
  - `tier-1-replaceable` (use `core::` or `alloc::` instead)
  - `tier-2-host-io` (legitimately needs `std`, belongs in
    `rubyrs-language` or above)
  - `tier-3-stdlib` (will move to `rubyrs-stdlib`)
- A **BigInt placement decision** recorded here: BigInt lives
  in `rubyrs-core` behind a default-on `bignum` Cargo feature,
  same shape as `regex`. `num_bigint` ships a `no_std` target
  (verified by reading its `Cargo.toml`), so this does not
  break the Phase 2 `no_std` ratchet (described below).

### Phase 1 — `rubyrs-core` extraction (the painful one)

Create `crates/rubyrs-core/` and move the Tier-1 modules into
it. Concretely:

- `parser.rs`, `ast.rs` (the rubyrs translation; Prism stays an
  external crate), `bytecode.rs`, `compiler.rs`, `vm.rs` and
  the entirety of `vm/`, `heap.rs`, `value.rs`, `intern.rs`,
  `error.rs`, plus the `Config`/`Runtime` embed API.
- Add `#![no_std]` + `extern crate alloc` to the crate root.
- Replace every `use std::` site flagged `tier-1-replaceable`
  with the audit's mapped `alloc::`/`core::` import.
- `rubyrs` (the CLI/facade crate) stays at `crates/rubyrs/`
  and shrinks to just `main.rs` + a paper-thin `lib.rs` that
  re-exports `rubyrs_core::*`. The extraction PR verifies two
  surfaces stay intact:
  - **Library API** (what `cargo add rubyrs` exposes) — diff
    `cargo doc --no-deps -p rubyrs` output before/after.
  - **CLI behaviour** (what `cargo install rubyrs` produces) —
    run the existing diff_cruby suite against the new binary;
    every fixture must still match CRuby byte-for-byte.
- `cext` feature stays in `rubyrs` (the facade) because cext is
  a Tier-3 capability; it must not be in `core` even behind a
  feature flag (Rule 3).

**Local freeze rule for Phase 1 only**: while the extraction PR
is open, no feature work that adds new *modules* to the soon-to-
be-extracted set. Adding methods to existing modules is fine —
those go where the module goes. Phase 1 should ship in **≤ 2
weeks** to keep the freeze short.

Phase 1 is the only phase that touches "everything"; all other
phases are additive.

### Phase 2 — CI ratchets land (immediately after Phase 1)

The three benchmarks ADR 0015 Rule 7 names become gating CI
checks. All three are measured on the **facade binary built with
the `core`-tier feature set only** (a library crate by itself
has no binary size; what we cap is the smallest shippable
artefact a user could `cargo install` once the tiers are split):

- **core-only binary size** ≤ 6 MB. ADR 0015's baseline was
  ~4 MB on the pre-PoC tree. The `regex` PoC (PR #86) measured
  a single-digit-MB save on the smallest viable build at merge
  time — concrete delta lives in that PR's description rather
  than here, since it shifts with toolchain versions. The 6 MB
  ceiling is what we lock in for the post-extraction
  `rubyrs-core`-derived binary.
- **core-only cold start** for `puts 1+2` ≤ 5 ms
- **core-only embed RSS** ≤ 8 MB

Plus a fourth, specific to this migration:

- **`no_std` ratchet**: enforcement is layered. The
  `#![no_std]` attribute on the `rubyrs-core` crate root
  removes `std` from the prelude, so accidental `use std::*`
  imports stop compiling on any target — that's the
  in-source contract, and it catches casual drift. It is
  *not* an absolute ban (someone determined could still write
  `extern crate std;` explicitly on a target where `std` is
  available). The hard guarantee comes from CI: PRs that
  touch `rubyrs-core` also have to compile on the bare-WASM
  target
  (`wasm32-unknown-unknown` — which genuinely has no `std`;
  `wasm32-wasip1` ships `std` and would silently let drift in)
  in **two passes** on every PR that touches `rubyrs-core`:
  - `cargo check -p rubyrs-core --no-default-features --target
    wasm32-unknown-unknown` — the pure minimum; catches direct
    `std` imports in core code
  - `cargo check -p rubyrs-core --target wasm32-unknown-unknown`
    (default features on) — catches `std` leaking in through a
    default-on dependency like `bignum` (`num_bigint`'s `std`
    feature is opt-out, so a future bump that flips it on by
    accident would otherwise sneak past the first pass)

  Layered together — `#![no_std]` rejects casual drift at the
  source, and the bare-WASM target rejects determined drift at
  build time (since `std` is absent on that target, even an
  explicit `extern crate std;` won't link). The two-pass
  feature variant catches transitive drift via default-on
  dependencies.

CI scripts live in `.github/workflows/`. Phase 2 is one PR.

### Phase 3 — `rubyrs-language` extraction

Move things that compile to bytecode but legitimately need
`std` into a new `rubyrs-language` crate:

- Process control (`Process`, `Kernel#system`, `Kernel#exec`,
  `Kernel#exit`)
- Full IO (today's `vm/fileops.rs` capability-gated wrappers
  graduate to first-class once they have a `std` home)
- Future: Fiber, Thread (when implemented)

This is additive — no extraction from `core`. Each migrated
feature is its own PR.

### Phase 4 — `rubyrs-stdlib` extraction

Pure-Ruby stdlib reimplementations (the slice already vendored
under `tests/diff/` from `msgpack/timestamp.rb` is the first
candidate). Crate exists as a vendor of `.rb` source files
plus a loader that the facade wires in via `Kernel#require`.

Each stdlib file is one PR.

### Phase 5 — `rubyrs-mri-compat` (deferred to v3+)

Per ADR 0015's "Tier 4 is a public bet, not a covenant", this
phase has no schedule. It exists in the workspace plan so that
its eventual landing has a slot reserved, but no PRs are
written for it inside the v1/v2 roadmap.

### Freeze policy summary

| Phase | Freeze | Duration | What's blocked |
|-------|--------|----------|----------------|
| 0 | None | 1 PR | nothing |
| 1 | Local | ≤ 2 weeks | new modules in core's extraction set; method-level additions OK |
| 2 | None | 1 PR | nothing |
| 3 | None | rolling | nothing |
| 4 | None | rolling | nothing |
| 5 | N/A (deferred) | — | — |

### BigInt landing position (settled here)

BigInt is **Tier-1 semantics with Tier-1 implementation**:
`Value::BigInt(ObjId)` lives in `rubyrs-core`, gated behind a
default-on `bignum` Cargo feature alongside `regex`. The PoC
plan in the BigInt thread can proceed in **parallel with Phase
1** because:

- `num_bigint` ships a `no_std + alloc` target — verified
  before this ADR was accepted.
- The `Value::BigInt` variant and `Op` arms drop cleanly into
  whichever crate holds `value.rs`/`bytecode.rs` at the time
  the PR lands.
- A BigInt PR opened during Phase 1 should target the
  pre-extraction layout (`crates/rubyrs/src/`); the extraction
  PR carries it through to `crates/rubyrs-core/src/` via the
  same `git mv` it applies to everything else.

**Phase B status — complete (2026-05-26).** All seven Phase B
groups shipped; the BigInt surface now covers every
`Integer`-protocol method that pure-Ruby code can reasonably
call on out-of-i64 values. Lifecycle by PR:

| Group | Surface | PR |
|---|---|---|
| B.1 | Base arithmetic, comparison, to_s/inspect, predicates, auto-promote/demote | — (pre-A) |
| B.2 | `-@` / `+@` / `abs` with i64::MIN promote | #121 |
| B.3 | `~` / `& | ^` / `<< >>` two's-complement bit ops + DoS cap | #159 |
| B.4 | `to_s(radix)` + sprintf `%d/%i/%b/%B/%o/%x/%X` + shared cap estimator | #138 |
| B.5 | `pow(exp[, mod])` + `bit_length` + `digits` | #123, #129 |
| B.6 | `times` / `upto` / `downto` block iteration | #174 |
| B.7 | `eql?` / `hash` / `Object#equal?` BigInt arm | #171 |

Two invariants the Phase B work codified, must be preserved by
any future change:

1. **Canonical-BigInt invariant.** Every `Value::BigInt(id)`
   that reaches dispatch has magnitude strictly outside
   `i64::MIN..=i64::MAX`. The sole funnel is `bigint_to_value`,
   which demotes-on-fit; every arm that produces a BigInt
   result MUST route through it. Debug-asserts in
   `try_bigint_unary`'s `+@` / `abs` identity short-circuits
   catch FFI bypasses.
2. **DoS-cap convention.** Pre-allocation estimators in every
   arm that can produce arbitrarily large output trap **before**
   the alloc when the estimated byte cost exceeds
   `Config::max_value_bytes` (fallback 1 MB, same as
   `try_bigint_pow`'s original). Two flavours, depending on what
   the arm is about to allocate:
   - **BigInt-allocation caps** — `try_bigint_pow` (result of
     `base ** exp`) and `try_bigint_bit_shift` (result of
     `recv << n` / `recv >> n`). Estimate rounds up to u64 limbs
     + 32-byte allocator header so the cap reflects actual heap
     storage, not just minimal bit count.
   - **String-formatting caps** — `check_bigint_to_s_cap`
     (BigInt#to_s output), `format_radix_any` (sprintf
     `%b/%B/%o/%x/%X` output), and the `%d/%i % big` path in
     `vm::sprintf`. Estimate is the rendered character count
     (digits + sign byte + optional `0x`/`0b` prefix). These
     bound the output String length, not the underlying BigInt
     storage.

Implementation invariants and call-graph diagrams live in
`crates/rubyrs/src/vm/bignum.rs`'s module doc — keep it
synchronised with this section.

## Consequences

### What gets easier

- **Embed users get a small core by default**. Post-Phase-1,
  `cargo add rubyrs-core` will pull in just the language — no
  cext, no regex, no host IO. This matches mruby's "tiny by
  default" niche.
- **WASM/embedded targets stay viable forever**. The `no_std`
  ratchet on CI means a PR that drags `std` into `core` cannot
  land. Today's lack of such a ratchet is silent permission for
  drift.
- **ADR 0015 Rule 3 ("no outer-tier hooks in core") becomes
  mechanically enforceable**. Once `rubyrs-core` does not
  depend on `rubyrs-language`, an outer-tier import simply
  won't compile. Today the rule lives only in code review.
- **The "could this run X?" question stops bottlenecking
  early-tier decisions**. Once Tier 2 exists as a crate,
  adding `Process.spawn` doesn't require a `tier-1-safe?`
  debate — the crate boundary is the answer.
- **PR conflicts drop**. Two feature PRs touching different
  tiers don't share a Cargo.toml or src directory.

### What gets harder

- **Phase 1 is unavoidable big-PR work**. Mitigation: the
  audit (Phase 0) lets us split it into "pure moves" + "import
  rewrites" sub-PRs if needed. Phase 1's local freeze caps the
  exposure to ≤ 2 weeks.
- **`#![no_std]` discipline in `rubyrs-core` is a forever
  cost**. Any future feature PR has to think about it, and
  the ratchet will reject PRs that don't. Mitigation: the
  audit document doubles as a "what to use instead" reference.
- **Embed users that today import `rubyrs::*` see no break**
  (the facade re-exports the same surface) — *but* anyone who
  depended on internal paths like `rubyrs::vm::*` will need
  to update. Mitigation: those paths are `pub(crate)` today,
  so external users shouldn't have them; auditable via
  `cargo doc --no-deps` diff in Phase 1.
- **Four crate boundaries means four `Cargo.toml` to keep in
  sync** (workspace versioning, edition, lints). Mitigation:
  `[workspace.package]` and `[workspace.lints]` already
  centralise most of this; the migration adds two new members
  to the inheritance.

### What we explicitly accept trading away

- **Phase 0/1 cost ~3 weeks of calendar time** with one PR
  open most of that. We accept this because the alternative is
  letting `std` drift unboundedly while the project grows past
  the size where extraction is still feasible.
- **The cext crate stays at top-level (`rubyrs-cext`) and does
  not move into a tier crate.** It's a Tier-3 capability but
  it's also a separate-process concern (C ABI), so giving it a
  tier label would muddy what "Tier 3" means. We accept the
  inconsistency in exchange for not over-engineering the
  cext story before there's a concrete need.
- **`rubund` and `rubyrs-gapscan` keep their current top-level
  position.** They consume `rubyrs` but aren't part of the
  tier stack — they're tools built on top of it. Naming them
  `rubyrs-tools-*` would be tidier but not load-bearing.

## Alternatives considered

- **Stay single-crate, layer with features only.** This is
  what we have today. Works mechanically (the cext + regex
  PoCs prove it) but leaves Rule 3 unenforceable; "no outer-
  tier hooks in core" becomes a code-review convention that
  drifts at every contributor turnover.
- **One mega-PR that does all phases.** Tried by Artichoke;
  it's the failure mode this ADR is structured to avoid.
- **Phase 1 + facade only; skip Phases 3–5 indefinitely.**
  Tempting because Phase 1 already buys most of the
  enforceability. But without `rubyrs-language` existing as a
  destination, the next time someone adds `Process.spawn`
  there's no obvious home and it ends up in `rubyrs-core`
  "for now" — exactly the drift we're avoiding.

## Open questions (to settle in Phase 0)

1. Does the `[workspace.lints]` table propagate `#![no_std]`?
   (Probably no — it's not a lint, it's a crate attribute.
   Will need per-crate enforcement.)
2. Should `rubyrs-core`'s `Config` struct grow a feature-gated
   `bignum: bool` to let embed users *runtime-disable* a
   feature their build pulled in? Or is build-time enough?
   Lean: build-time only — runtime knobs proliferate.
3. Where does the `Runtime::register_fn` v2 host-API live?
   Today it's in `vm.rs`; arguably it's Tier-2 (the API only
   matters if you're embedding from outside). Lean: it stays
   in `rubyrs-core` because the *mechanism* (HostFnSlot
   dispatch) is Tier-1 even if typical *consumers* aren't.
