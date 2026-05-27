# 0019: Tier 2 / Tier 3 boundary specification

## Status

Proposed (2026-05).

## Context

[ADR 0015](0015-concentric-architecture.md) committed rubyrs to a
four-tier concentric shape. [ADR 0017](0017-tier1-boundary.md)
locked the **inside** of the inner ring: the four rules for what
counts as Tier 1.

ADR 0015's outer-tier sketch was:

- **Tier 2 (`language`)** — full Ruby semantics, Sinatra/Rack
  capable.
- **Tier 3 (`stdlib`)** — pure-Ruby gems, CLI tools.

That sketch is fine as a marketing slogan, but it's not a spec.
Three concrete pressures push us to draw the line:

1. **rubund** (Bundler-in-Rust, already in the workspace) needs
   more than Tier 1 today and is structurally importing the whole
   `rubyrs` crate. It will eventually depend on Tier 2 or Tier 3 —
   but which?
2. **The Bun-class question.** Native SQLite / HTTP / S3 /
   WebSocket modules are the obvious differentiation lever (see
   the project-positioning discussion at session-level). They are
   **not language features**, but they are also **not pure
   Ruby**. The naive "Tier 3 = pure Ruby" reading boxes them out
   of every tier — that's wrong, and freezes the strategic move.
3. **First-Tier-3-PR risk.** Without ADR 0019, the first
   contributor (or AI agent) to open a Tier 3 PR will re-litigate
   the entire boundary in their review. We saw this with the
   cext PoC pre-0017: ~13 cfg sites across 6 files, all bikeshed
   because the rules weren't written down. The fix is the same:
   write the rules down before the PRs land.

ADR 0017's "no script-accessible OS capabilities by default"
already removed File / Net / Process / `system` from Tier 1.
ADR 0019 has to answer where each of them goes next — and where
the Rust-backed batteries (SQLite, HTTP, S3) sit.

## Decision

Adopt **implementation-locus** as the primary axis for the
Tier 2 / Tier 3 boundary. A feature is Tier 2 if it requires
changes to the **interpreter** (VM ops, GC layout, object model,
parser/compiler surface). It is Tier 3 if it ships as
**require-able code** — Ruby source files vendored under
`rubyrs-stdlib`, optionally backed by Rust modules that expose
their surface through the existing host-fn / cext mechanism but
do **not** change VM behaviour.

This replaces the looser "Rust vs Ruby" framing, which would
leave Rust-implemented batteries (SQLite, HTTP, S3) homeless.

### The Tier 2 / Tier 3 rules

1. **Tier 2 owns runtime behaviour.** If shipping the feature
   requires editing `vm/*.rs`, adding bytecode ops, changing
   the `Value` enum, adding a heap variant, growing the GC
   root set, or extending the parser/compiler — it's Tier 2.
   `Fiber`, `Thread`, `Ractor`, `ObjectSpace`, `Marshal`,
   string `eval` with full lexical scope capture, full
   metaprogramming reflection — all Tier 2 because each
   demands new runtime state or new opcodes.

2. **Tier 3 owns library surface.** If the feature can land as
   `.rb` source files (vendored in `rubyrs-stdlib/lib/`) and/or
   a Rust module that registers host fns / cext classes through
   APIs the runtime already exposes — it's Tier 3. `json`,
   `csv`, `yaml`, `logger`, `optparse`, `tempfile`, `Date`, plus
   the **Rust-backed batteries** (SQLite, HTTP, S3, WebSocket,
   …) all live here. The discriminator is "does the runtime
   need to know this exists?" — Tier 3 answer is no.

3. **Tier 3 batteries are individually opt-in.** No
   monolithic `stdlib` feature that drags in everything. Each
   battery gets its own `_<name>` feature
   (`_sqlite`, `_http`, `_s3`, `_websocket`, `_json`, `_csv`,
   …). The aggregate `stdlib` feature exists for convenience
   but enables only the "pure-Ruby canon" set (`json`, `csv`,
   `yaml`, `logger`, `optparse`, `tempfile`, `set`, `Date`) —
   batteries that touch the network, the filesystem, or
   native bindings each require their own explicit opt-in.

4. **Tier 3 batteries respect capability gating.** Even
   Rust-backed batteries route their OS-touching calls
   through the existing capability mechanisms (host-fn
   injection or cext): `_http`'s `fetch` ultimately calls a
   host fn the embedder registered for network access, not
   a direct `reqwest::get`. The exception is **batteries
   that own their own file** (canonically: SQLite databases
   the user opens by path) — those bypass the capability
   model and document the deviation per battery. The
   deviation policy is: a battery may bypass capability
   gating only if (a) it's explicitly opt-in via its
   `_<name>` feature, (b) the embedder can refuse to enable
   it for sandbox builds, and (c) the deviation is recorded
   in that battery's own ADR.

5. **Dependency direction is strict.** Tier 3 may import
   Tier 2 may import Tier 1. The reverse is forbidden. This
   is the same Rule 3 from ADR 0015 applied one ring out:
   "no outer-tier hooks in core code."

6. **Each Tier 3 battery gets its own ADR.** Short (~50–100
   lines): the vendor crate choice (`rusqlite` vs
   `sqlx-sqlite`; `reqwest` vs `ureq` vs `hyper`), the
   capability boundary (which OS resources does it touch,
   what's gated through host fns, what's bypassed), the
   surface freeze policy (which Ruby methods are stable
   first-release API), and the failure-mode mapping (how do
   Rust-side errors surface as Ruby exceptions). This keeps
   the per-battery cost legible and prevents "stdlib
   creep" — every PR for a new battery has to argue its case
   in its own ADR before the cfg sites land.

### Implementation locus matrix

| Feature | Tier | Implementation locus | Why |
|---|---|---|---|
| `Fiber` | 2 | `rubyrs-language` — new VM state | Coroutine stack, switch op |
| `Thread`, `Mutex`, `Queue` | 2 | `rubyrs-language` — new runtime + GC | OS-thread integration; out-of-tier-1 by Rule 4 |
| `Ractor` | 2 | `rubyrs-language` | Isolated VM per ractor |
| `ObjectSpace` (full) | 2 | `rubyrs-language` — weak-ref table on GC | Reflection into shared global state |
| `Marshal` | 2 | `rubyrs-language` — needs internal type knowledge | Object-graph serialisation w/ class identity |
| Full `eval(string)` w/ lexical scope | 2 | `rubyrs-language` — re-entrant parser + scope splice | Tier 1's `eval` is class-body only; full form is Tier 2 |
| `TracePoint`, `set_trace_func` | 2 | `rubyrs-language` — instruction-level hooks | VM-level instrumentation surface |
| `Date`, `DateTime` | 3 | `rubyrs-stdlib` — pure Ruby on top of injected `Time.now` | No VM change needed |
| `JSON` | 3 | `rubyrs-stdlib` — pure Ruby (or upgrade to native via `_json_native`) | flori_json cext already loads; native opt-in possible |
| `CSV` | 3 | `rubyrs-stdlib` — pure Ruby | Library code |
| `YAML` (psych) | 3 | `rubyrs-stdlib` + `_yaml` battery | `serde_yaml` or `yaml-rust2` Rust backend |
| `Logger`, `OptionParser` | 3 | `rubyrs-stdlib` — pure Ruby | Library code |
| `Tempfile`, `FileUtils` | 3 | `rubyrs-stdlib` + capability gate | Pure Ruby wrapping `File` host fns |
| `Set` | 3 | `rubyrs-stdlib` — pure Ruby on top of Hash | Pure data structure |
| **SQLite (native)** | **3** | **`rubyrs-stdlib` + `_sqlite` battery** (`rusqlite`) | Rust-backed battery; owns its own file (Rule 4 deviation, allowed via own ADR) |
| **HTTP fetch (native)** | **3** | **`rubyrs-stdlib` + `_http` battery** (`reqwest` or `ureq`) | Rust-backed battery; routes through host capability |
| **S3 (native)** | **3** | **`rubyrs-stdlib` + `_s3` battery** (`aws-sdk-s3`) | Same shape as HTTP |
| **WebSocket** | **3** | **`rubyrs-stdlib` + `_websocket` battery** | Network capability; Rust-backed |
| `Net::HTTP` (pure Ruby) | 3 | `rubyrs-stdlib` — pure Ruby on top of `_http` | Compat shim atop the native battery |
| `OpenSSL` (low-level) | 3 | `rubyrs-stdlib` + `_openssl` battery (`rustls`) | Crypto primitives; Rust-backed |
| `File`, `Dir`, `IO` | 3 | `rubyrs-stdlib` + `_io` battery + capability gate | Capability-gated host fns; pure-Ruby class layer on top |
| C extension ABI (full) | 4 | `rubyrs-mri-compat` (out of 0019 scope) | Deferred per ADR 0015 Rule 6 |
| Rails / ActiveRecord | 4 | (multi-year bet) | Out of 0019 scope |

### What changes vs ADR 0015's sketch

| Question | ADR 0015 said | ADR 0019 says |
|---|---|---|
| Where do Rust-backed batteries live? | Implicit "stdlib = pure Ruby" | Tier 3 with individual opt-in features |
| Can Tier 3 contain Rust code? | Ambiguous | Yes — vendored Ruby + Rust modules side-by-side |
| Is `_sqlite` Tier 2 or Tier 3? | Undefined | Tier 3 — doesn't change VM |
| Is `_fiber` Tier 2 or Tier 3? | Implicit Tier 2 | Tier 2 explicit — needs runtime state |
| One `stdlib` feature or many? | One umbrella | One umbrella **plus** per-battery `_<name>` features |
| Does each battery need an ADR? | Unspecified | Yes — Rule 6 |

ADR 0015's tier table stays correct; ADR 0019 makes the
"implementation locus" axis explicit so the table can be
applied to specific PRs.

### What this is not

- **Not a roadmap.** ADR 0019 specifies the boundary, not the
  order. Whether SQLite, HTTP, or `Fiber` ships first is
  release-planning, not architecture.
- **Not a license to skip ADR 0018's phase ordering.** Tier 2 /
  Tier 3 work still happens **after** the Phase 1
  `rubyrs-core` extraction. ADR 0019 doesn't reshape the
  migration; it specifies what gets extracted into
  `rubyrs-language` and `rubyrs-stdlib` when their phases
  arrive.
- **Not a permanent capability-gating waiver for native
  batteries.** Rule 4's deviation clause applies per battery
  via its own ADR — the policy default is "respect gating."
- **Not a freeze on language semantics.** Tier 1 still gets
  feature work (the diff_cruby gap-reports continue). ADR 0019
  only constrains *where* outer-tier work lands.

## Consequences

### What gets easier

- **First Tier 3 PR has a home.** `_sqlite`, `_http`, `_s3`
  each have a pre-decided crate layout and a written rule for
  capability-gating. The PR is "implement the battery", not
  "argue the boundary."
- **rubund's import shape becomes clear.** It depends on Tier
  2 (`rubyrs-language`) — needs full IO and Process — not on
  Tier 3. This unblocks rubund's structural placement once
  Phase 1+3 of the workspace migration land.
- **Bun-class story has architectural backing.** "Native SQLite
  shipped in the binary" is a credible claim because we have a
  named tier slot, a vendor crate convention, and a written
  rule for the capability deviation. The slot exists; what's
  missing is execution.
- **Per-battery cost stays bounded.** ADR-per-battery (Rule 6)
  + per-battery feature flag means a SQLite PR can't drag in
  HTTP design choices or v.v. The discipline is the same as
  the Cargo-features convention from ADR 0015 — just one ring
  out.

### What gets harder

- **More ADRs.** Each Tier 3 battery wants ~50–100 lines of its
  own ADR. Estimate: 4–6 batteries in the v2 timeframe → 4–6
  new ADRs. This is the price of legibility; the alternative
  (one ADR covering "the stdlib") drifts into stale-doc
  territory within a release.
- **Cargo feature matrix grows.** Today: `cext`, `regex`,
  `bignum`, `stdlib` (pending). After ADR 0019 + first three
  batteries: + `_sqlite`, `_http`, `_s3`. CI matrix has to
  exercise each combination (or a representative subset).
  ADR 0015 Rule 7's three ceiling metrics expand to per-battery
  cost accounting.
- **Pure-Ruby vs Rust-backed parity questions surface.** When
  both `JSON` (pure Ruby in `rubyrs-stdlib`) and `_json_native`
  exist, the embedder picks. CRuby has the same shape (`json`
  vs `json/pure`) so this is solved-elsewhere territory, but
  the documentation cost is real.
- **Tier 3 batteries can be load-bearing for marketing while
  being shallow in implementation.** "rubyrs ships SQLite!" is
  tempting to announce at the first PR that loads `rusqlite`.
  The Rule 6 ADR requirement plus "surface freeze policy"
  language is the brake — first PR doesn't get to claim a
  stable Ruby API surface.

### What we explicitly accept trading away

- **The "pure-Ruby stdlib" purist position.** Some Ruby
  community subset will read "Tier 3 ships Rust code" as
  betraying the Ruby aesthetic. We accept this. The
  alternative — Bun-class native batteries living outside the
  tier system as ad-hoc crates — costs more architectural
  coherence than it saves.
- **A single `stdlib` flag.** Embedders who want "all the
  things" will have to enumerate features. The alternative is
  silent dependency creep (a `stdlib` flag that grows over
  time changes binary size unannounced). Verbose-explicit
  wins; ADR 0015 Rule 2 ("opt-in, not opt-out") backs us up.
- **Symmetric implementation across batteries.** Each battery
  gets to choose its own crate (rusqlite vs sqlx; reqwest vs
  ureq vs hyper) and its own surface. We don't enforce a "all
  batteries use trait Backend" abstraction — that's premature
  abstraction in a domain (network / disk / native bindings)
  where each crate has its own idiom.

## Alternatives considered

1. **"Tier 3 = pure Ruby only" boundary.** Clean and easy to
   explain. Boxes out the Bun-class differentiation move. Forces
   Rust-backed batteries into either Tier 2 (wrong — they don't
   change VM) or out-of-tier crates (loses the workspace
   coherence ADR 0015 was built to preserve).

2. **"Tier 3 = pure Ruby; new Tier 5 = native batteries"
   five-tier model.** Splits the difference syntactically but
   adds a ring to ADR 0015's four-ring picture. Pays
   conceptual debt to dodge a paragraph of explanation. We
   reject the extra tier on legibility grounds.

3. **"One Tier 3 feature drags in everything."** Convenient at
   build time but loses the per-battery cost accounting.
   `cargo install rubyrs --features stdlib` would suddenly
   carry `reqwest` (8 MB), `aws-sdk-s3` (15 MB+),
   `rusqlite` (4 MB) — for an embedder who wanted CSV. Kills
   the "5 MB embed" pitch.

4. **"No native batteries; bind via host fns only."** Forces
   embedders to wire up SQLite themselves via
   `register_fn_v2`. Architecturally pure (capability gating
   100% intact) but loses the Bun-class lever — "rubyrs ships
   batteries included" can't be said. The compromise in
   Rule 4 (deviation allowed via per-battery ADR) is the
   pragmatic middle.

5. **"Pure-Ruby native shim layer in Tier 2."** Have the
   runtime expose a fixed set of primitives (e.g.
   `Native.sqlite_open(path)`) and write the Ruby-side
   `SQLite3::Database` class in Tier 3 pure Ruby. Two-layer
   discipline. Adds bilateral coupling: any new battery
   needs both a Tier 2 primitive AND Tier 3 Ruby. Doubles
   the per-battery cost relative to "Tier 3 owns the whole
   battery." Considered and rejected; the simpler version of
   "battery is one place" wins.

## Related

- [ADR 0015 — Concentric architecture](0015-concentric-architecture.md)
  — the four-tier shape this ADR refines.
- [ADR 0017 — Tier-1 boundary specification](0017-tier1-boundary.md)
  — the inner-ring spec; ADR 0019 is the same exercise one ring out.
- [ADR 0018 — Workspace migration plan](0018-workspace-migration.md)
  — the phased path to land the multi-crate split. ADR 0019
  shapes what Phase 3 (`rubyrs-language` extraction) and Phase 4
  (`rubyrs-stdlib` extraction) carry.
- [ADR 0007 — Host embedding API](0007-host-embedding-api.md) —
  the `Runtime` / `Config` / `register_fn` surface that Tier 3
  batteries route through when they respect capability gating
  (Rule 4 non-deviation path).
- [ADR 0009 — cext panic policy](0009-cext-panic-policy.md) —
  the cext ABI used by msgpack / flori_json / bcrypt is itself
  a Tier 4 mechanism, but the policy applies to any Tier 3
  battery that opts into cext-style native code.
