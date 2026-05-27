# 0019: Tier 2 / Tier 3 boundary specification

## Status

Proposed (2026-05). Revised 2026-05-27 after parallel review (see
"Revision log" at the bottom). Not yet accepted; supersedes its own
v1 draft, which lives in git history at commit `d53b044a`.

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
Four concrete pressures push us to draw the line:

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
4. **Binary shape vs tier are independent.** ADR 0015 collapsed
   them — "tier" controlled both language capability AND binary
   shape. After review (Q1 of the 2026-05-27 discussion), we
   recognise these are orthogonal axes: a user installing the
   CLI wants batteries-included even if the library default
   stays opt-in.

ADR 0017's "no script-accessible OS capabilities by default"
already removed File / Net / Process / `system` from Tier 1.
ADR 0019 has to answer where each of them goes next, where
the Rust-backed batteries (SQLite, HTTP, S3) sit, and how the
"one binary all batteries" Bun shape coexists with the 5 MB
embed pitch.

## Decision

### Part A — the implementation-locus axis

Adopt **implementation-locus** as the primary axis for the
Tier 2 / Tier 3 boundary. A feature is Tier 2 if it requires
changes to the **interpreter** (VM ops, GC layout, parser/
compiler surface, object model). It is Tier 3 if it ships as
**require-able code** — Ruby source files vendored under
`rubyrs-stdlib`, optionally backed by Rust modules that
register through existing host-fn / cext APIs without
changing VM behaviour.

This replaces the looser "Rust vs Ruby" framing, which would
leave Rust-implemented batteries (SQLite, HTTP, S3) homeless.

### Part B — the eight rules

1. **Tier 2 owns runtime behaviour.** If shipping the feature
   requires editing `vm/*.rs`, adding bytecode ops, changing
   the `Value` enum, adding a heap variant, growing the GC
   root set, or extending the parser/compiler — it's Tier 2.
   `Fiber`, full string `eval` with lexical scope capture,
   `TracePoint`, complete metaprogramming reflection — all
   Tier 2 because each demands new runtime state or new
   opcodes.

2. **Tier 3 owns library surface.** If the feature can land
   as `.rb` source files (vendored in `rubyrs-stdlib/lib/`)
   and/or a Rust module that registers host fns / cext
   classes through APIs the runtime already exposes — it's
   Tier 3. `json`, `csv`, `yaml`, `logger`, `optparse`,
   `tempfile`, `Date`, plus the **Rust-backed batteries**
   (SQLite, HTTP, S3, WebSocket, OS threads, …) all live
   here. The discriminator is "does the runtime need to know
   this exists?" — Tier 3 answer is no.

3. **Tier 3 batteries are individually opt-in.** No
   monolithic `stdlib` feature that drags in everything.
   Each battery gets its own `_<name>` feature
   (`_sqlite`, `_http`, `_s3`, `_websocket`, `_thread`,
   `_ractor`, `_yaml`, `_csv`, `_json_native`, …). The
   aggregate `stdlib` feature exists for convenience but
   enables only the **pure-Ruby canon** set
   (`json`, `csv`, `yaml`, `logger`, `optparse`, `tempfile`,
   `set`, `Date`) — batteries that touch the network, the
   filesystem, OS threads, or native bindings each require
   their own explicit opt-in.

   **Every `_<name>` feature is the sole enabler for its
   vendor crate.** No transitive accidental activation; the
   wasmtime `dep:` pattern from ADR 0015's Cargo.toml
   example is normative here.

4. **Tier 3 batteries respect capability gating by default.
   Deviations follow a fixed taxonomy.** Each battery
   declares which deviation class(es) it claims in its own
   ADR. The taxonomy:

   | Deviation class | What it covers | Admissible? |
   |---|---|---|
   | **a. Owned-resource I/O** | Battery operates on a resource the user explicitly hands it (path, URL, descriptor) and does nothing else | Yes — canonical for `_sqlite`, `_http` with caller-supplied URL |
   | **b. Local subprocess** | Battery spawns child processes (`_git` shelling out, `_ffmpeg`) | Conditional — only if no shell-injection vector and only opt-in |
   | **c. Network reach beyond a single URL** | Battery makes network calls beyond the caller's literal arg (e.g. follows redirects to other hosts, DNS prefetch, retries to mirrors) | Conditional — must document allowlist policy |
   | **d. Filesystem walk beyond a single path** | Battery enumerates directory trees from a starting point (`_git`'s `.git/` walk, `_find`) | Conditional — must document scope bound |
   | **e. Time / entropy source** | Battery reads wall clock or system entropy (`_chrono` NTP, native PRNG) | Yes — non-deterministic, but documented |
   | **f. Memory-map / shared memory** | Battery uses `mmap`, shared memory, or other heap-cap-bypassing allocation | Conditional — must self-impose a cap honouring `Config::max_value_bytes` |
   | **g. Native-thread spawn** | Battery creates OS threads or thread pools (`_thread`, `_ractor`, network batteries that use `tokio` internally) | Yes — Tier 3 by definition; cannot exist in Tier 1/2 |

   **Inadmissible deviation classes** (must NOT be claimed):
   - **Capability bypass via env-var trapdoors** (e.g. a
     battery that respects gating "unless `RUBYRS_BYPASS=1`
     is set"). No env-var trapdoors. Period.
   - **Privilege escalation** (e.g. a battery that touches
     `/etc/`, `/sys/`, `/proc/` without an opt-in flag).
   - **Cross-battery state leakage** (e.g. `_sqlite` reading
     a path from `_http`-fetched content without the user
     mediating).

   The per-battery ADR (Rule 7 below) MUST list which
   deviation classes apply. CI grep can verify the ADR
   contains a `## Deviations` section enumerating from the
   table above. The list above is the closed taxonomy for
   v0.x — adding a class requires amending ADR 0019.

5. **Dependency direction is strict.** Tier 3 may import
   Tier 2 may import Tier 1. The reverse is forbidden. This
   is the same Rule 3 from ADR 0015 applied one ring out:
   "no outer-tier hooks in core code."

   **Intra-Tier-3 dependencies are allowed but flat-only.**
   A Tier 3 pure-Ruby module may depend on a Tier 3 native
   battery (e.g. `Net::HTTP` pure-Ruby on top of `_http`),
   but cycles between batteries are forbidden, and a
   battery may not import a sibling battery silently — the
   dependency must appear in the depender's ADR.

6. **Pure-Ruby is canonical; native is the accelerator.**
   When a battery has both shapes (e.g. JSON pure-Ruby AND
   `_json_native`), the pure-Ruby implementation is the
   reference. The native battery is a drop-in
   *performance* upgrade; its observable behaviour
   (results, error shapes, edge cases) must match the
   pure-Ruby version. The native PR is required to
   pass the pure-Ruby's test suite. If a divergence is
   intentional, the native battery's ADR records it as a
   deviation.

   This is the **Deno stdlib-on-JSR direction** — pure
   forms work everywhere, native forms are the
   build-time optimisation. Embedders who care about
   semantics get the pure form's portability; embedders
   who care about throughput add `--features _json_native`.

7. **Each Tier 3 battery gets its own ADR.** Short (~50–100
   lines):
   - **Vendor crate choice** (`rusqlite` vs `sqlx-sqlite`;
     `reqwest` vs `ureq` vs `hyper`)
   - **Deviation list** (from Rule 4's taxonomy)
   - **Surface freeze policy** — see ADR 0021 (TBD)
     template; baseline: Ruby methods are `unstable` until
     the battery has shipped in one tagged release, then
     `stable` (semver-tracked). Removing a `stable` method
     requires a new ADR.
   - **Error mapping** — how do Rust-side errors surface
     as Ruby exception classes?
   - **Capability host-fns it consumes** (so embedders know
     which `register_fn_v2` slots to wire if they want to
     gate the battery's reach)

   This is enforced mechanically: CI fails any PR that
   adds a `_<name>` Cargo feature without a matching
   `docs/adr/00XX-battery-<name>.md` file (~20 lines of
   bash, paralleling the existing per-file panic budget at
   `.github/workflows/ci.yml`).

8. **Namespace convention for Rust-backed batteries.**
   Native batteries expose themselves under
   `require "rubyrs/<name>"`, NOT bare `require "<name>"`.
   This prevents silent shadowing of MRI gems once Tier 4
   compat lands (`require "sqlite3"` must continue to
   resolve to the gem if the gem is loadable; `require
   "rubyrs/sqlite"` is the explicit "give me the built-in
   native battery" form).

   Pure-Ruby Tier 3 batteries (`require "json"`,
   `require "csv"`) keep bare names — they ARE the
   MRI-shape API by design, so collision is non-existent.

   Rationale: Node solved this exact problem with `node:`
   prefix mandatory on its native built-ins
   (`node:sqlite`, `node:test`). The Ruby analogue keeps
   `rubyrs/` as the prefix.

### Part C — shape aliases (orthogonal to tier)

ADR 0015 Rule 2 says "default features include `core`
only." This applies to **library consumers** (`cargo add
rubyrs-core` or `cargo add rubyrs`). It does NOT apply to
**CLI consumers** (`cargo install rubyrs`), who legitimately
expect a working tool out of the box. Bun and Deno set the
expectation; we follow it.

Three shape aliases, defined as Cargo feature sets in the
`rubyrs` facade crate:

| Shape | Triggered by | Includes | Target user | Size budget |
|---|---|---|---|---|
| **embed** | `cargo add rubyrs-core` (or `rubyrs --no-default-features`) | Tier 1 only | Embedding Ruby in Rust hosts; WASM | ≤ 6 MB (ADR 0015 Rule 7) |
| **cli-defaults** | `cargo install rubyrs` (facade default) | Tier 1 + Tier 2 + Tier 3 pure-Ruby canon | Ruby CLI tool authors | ≤ 25 MB |
| **everything** | `cargo install rubyrs --features everything` | All tiers + all native batteries (SQLite, HTTP, S3, WS, OpenSSL, ...) | Bun-class CLI / Edge runtime | ≤ 150 MB |

Cargo shape:

```toml
[features]
# Library default (Rule 2 of ADR 0015 applies)
default = []                            # core only

# Aggregates (alphabetical for diffability)
language   = ["_fiber", "_full_eval", "_metaprog_extra", "_tracepoint"]
stdlib     = [                          # pure-Ruby canon only
    "language",
    "_pure_json", "_pure_csv", "_pure_yaml", "_pure_logger",
    "_pure_optparse", "_pure_tempfile", "_pure_set", "_pure_date",
]

# Shape aliases (CLI defaults, NOT library defaults)
cli-defaults = ["stdlib"]               # facade crate's default
everything   = [
    "cli-defaults",
    "_sqlite", "_http", "_s3", "_websocket",
    "_openssl", "_yaml_native", "_csv_native", "_json_native",
    "_thread", "_io",
]

# Cross-cutting toggles
sandbox = []                            # capability gating; orthogonal
wasm    = []                            # ensure no syscall paths slip in
```

The facade crate (`crates/rubyrs/Cargo.toml`) sets its own
`default = ["cli-defaults"]`. The library crate
(`crates/rubyrs-core/Cargo.toml`) sets `default = []`.

This is the wasmtime pattern (CLI binary defaults differ
from library defaults) and the convention Cargo's own
documentation endorses.

### Part D — binary-size budgets per shape

ADR 0015 Rule 7 sets the **core-only** ceilings (6 MB
binary, 5 ms cold start, 8 MB embed RSS). ADR 0019 adds
per-shape ceilings so the Bun-class story has measured
backing:

| Shape | Binary size | Cold start | RSS for `puts 1+2` |
|---|---|---|---|
| **embed** | ≤ 6 MB | ≤ 5 ms | ≤ 8 MB |
| **cli-defaults** | ≤ 25 MB | ≤ 15 ms | ≤ 20 MB |
| **everything** | ≤ 150 MB | ≤ 40 ms | ≤ 60 MB |

CI gates each shape (`scripts/check-shape-budgets.sh` runs
per shape, matching the existing peak-RSS ratchet pattern in
`.github/workflows/ci.yml`).

Per-battery delta accounting: each battery's own ADR
records the binary-size delta its `_<name>` feature adds
to the `cli-defaults` baseline. This makes the cost
visible per-PR rather than per-release.

### Part E — implementation-locus matrix (revised)

| Feature | Tier | Implementation locus | Notes |
|---|---|---|---|
| **Concurrency — cooperative** | | | |
| `Fiber`, `Enumerator` | 2 | `rubyrs-language` — new VM state | Coroutine stack, switch op |
| Fiber scheduler hook ABI | 2 | `rubyrs-language` — pluggable ABI | Tier 3 batteries (`_async_io`) plug in here |
| **Concurrency — OS** | | | |
| `Thread`, `Mutex`, `Queue`, `ConditionVariable` | **3** (`_thread`) | `rubyrs-stdlib + _thread battery` | Deviation class `g`. Embed/wasm builds omit. **Resolves conflict with ADR 0017** by moving to Tier 3 |
| `Ractor` | **3** (`_ractor`) | `rubyrs-stdlib + _ractor battery` | Isolated VM per ractor; deviation class `g` |
| **Reflection** | | | |
| Bounded `ObjectSpace.each_object(Class)` | 2 | `rubyrs-language` — GC root walk | Limited surface; weak-ref table NOT included |
| `ObjectSpace` weak-ref tables, full reflection | **4** | `rubyrs-mri-compat` | **Resolves conflict with ADR 0017**: full surface stays Tier 4 as 0017 says; limited form available at Tier 2 |
| `TracePoint`, `set_trace_func` | 2 | `rubyrs-language` — instruction-level hooks | VM-level instrumentation surface |
| `RubyVM::InstructionSequence` | 4 | `rubyrs-mri-compat` | CRuby-ABI parity, not language semantics |
| **Serialisation** | | | |
| `Marshal` (read + write Ruby's wire format) | 4 | `rubyrs-mri-compat` | Needs class-identity preservation across processes; tied to CRuby ABI semantics |
| `Marshal`-shape serialisation (rubyrs's own, not MRI-compatible) | 3 | `rubyrs-stdlib` | If we ever ship it; not currently planned |
| **Encoding** | | | |
| Encoding tables, `String#encoding`, `force_encoding` | 1 or 2 | TBD (see "Open question" below) | Strings carry encoding state in CRuby; rubyrs today is UTF-8 only |
| `_encoding_full` (all CRuby encodings) | 3 | `rubyrs-stdlib + _encoding_full battery` | Adds ~1 MB of conversion tables |
| **Metaprogramming** | | | |
| `define_method`, `method_missing`, `const_missing` | 1 | already shipped | Tier 1; ADR 0010 PoC done |
| `Module#prepend`, `Module#refine` (refinements) | 2 | `rubyrs-language` | Refinements require lexical scope tracking in method lookup |
| `BasicObject#instance_eval`, `Module#class_eval` (string form) | 2 | `rubyrs-language` (depends on `_full_eval`) | Re-entrant parser; closure scope splice |
| `alias_method`, `undef_method` | 1 | already shipped | |
| **eval family** | | | |
| `eval(<class-body string>)`, `class_eval(string)` | 1 | already shipped (limited) | Re-entrant parser, no lexical-scope splice |
| Full lexical-scope `eval` (binding capture, locals access) | 2 | `rubyrs-language + _full_eval` | The hard form; needs scope chain on the VM |
| **Library — pure Ruby** | | | |
| `Date`, `DateTime` | 3 | `rubyrs-stdlib` pure Ruby on top of injected `Time.now` | No VM change |
| `JSON` (pure) | 3 | `rubyrs-stdlib` pure Ruby | Canonical per Rule 6 |
| `CSV` (pure) | 3 | `rubyrs-stdlib` pure Ruby | |
| `Logger`, `OptionParser`, `Set` | 3 | `rubyrs-stdlib` pure Ruby | |
| `Tempfile`, `FileUtils` (pure-Ruby layer) | 3 | `rubyrs-stdlib` pure Ruby on top of `_io` host fns | Pure-Ruby class layer; relies on `_io` for actual disk reach |
| **Library — Rust-backed batteries** | | | |
| `_json_native` (RapidJSON / serde_json) | 3 | `rubyrs-stdlib + _json_native battery` | Accelerator per Rule 6; behaviour-equivalent to pure JSON |
| `_yaml_native` (`yaml-rust2` or `serde_yaml`) | 3 | `rubyrs-stdlib + _yaml_native battery` | Same shape as `_json_native` |
| **SQLite** | **3** | **`rubyrs-stdlib + _sqlite battery`** (`rusqlite`) | Deviation class `a`. `require "rubyrs/sqlite"` |
| **HTTP fetch** | **3** | **`rubyrs-stdlib + _http battery`** (`reqwest` or `ureq`) | Deviation class `a` for caller-URL; `c` if redirects enabled |
| **S3** | **3** | **`rubyrs-stdlib + _s3 battery`** (`aws-sdk-s3`) | Deviation class `c` (multi-host) + `g` (tokio threads) |
| **WebSocket** | **3** | **`rubyrs-stdlib + _websocket battery`** | Deviation class `a` for caller-URL |
| `OpenSSL` (low-level crypto) | 3 | `rubyrs-stdlib + _openssl battery` (`rustls`) | Deviation class `e` (RNG); via `rustls` defaults |
| **IO (capability-gated)** | | | |
| `Runtime::set_stdout`, `register_fn` for files | 1 | already shipped | Tier 1 capability-injection mechanism remains; ADR 0017 path preserved |
| `File`, `Dir`, `IO` Ruby classes | 3 | `rubyrs-stdlib + _io battery` | Pure-Ruby class layer on top of Tier 1 host-fn primitives. The Tier 1 host-fn mechanism is the ground floor; `_io` is the Ruby-class veneer |
| `Pathname` | 3 | `rubyrs-stdlib` pure Ruby on top of `_io` | |
| **Process** | | | |
| `Kernel#system`, `` Kernel#` ``, `exec`, `spawn` | 3 | `rubyrs-stdlib + _process battery` | Deviation class `b` |
| `Process.kill`, `Process.wait`, signal handling | 3 | `rubyrs-stdlib + _process battery` | |
| **CRuby compat surface** | | | |
| C extension ABI (`require 'foo.so'`) | 4 | `rubyrs-mri-compat` | Out of 0019 scope |
| `Fiddle`, `DL` | 4 | `rubyrs-mri-compat` | Out of 0019 scope |
| Rails / ActiveRecord | 4 | (multi-year bet) | Out of 0019 scope |

### Open question (to be resolved before 0019 ratification)

**Encoding placement.** Strings in CRuby carry an encoding
field. Today's `Value::Str` is UTF-8 only. Adding encoding
state is a Tier 1 decision (touches `Value` layout) or a
Tier 2 decision (Tier 1 stays UTF-8, Tier 2 layers
multi-encoding on top). ADR 0019 marks it "TBD" — a
follow-up ADR (or amendment to ADR 0017) resolves before
the first Tier 3 battery that produces non-UTF-8 bytes
ships (`_csv` reading Latin-1 files is the realistic
forcing case).

### What changes vs ADR 0019 v1

| v1 said | v2 says | Reason |
|---|---|---|
| `Thread`, `Mutex`, `Queue` → Tier 2 | **→ Tier 3 `_thread` battery** | Resolves direct conflict with ADR 0017; aligns with embed/wasm story; matches every embeddable runtime's industry default (mruby, Lua, rhai, rune) |
| `Ractor` → Tier 2 | **→ Tier 3 `_ractor` battery** | Same rationale |
| `ObjectSpace (full)` → Tier 2 | **Split: bounded form → Tier 2; full → Tier 4 (matches ADR 0017)** | Resolves direct conflict with ADR 0017; full surface is CRuby ABI parity, not language semantics |
| `Marshal` → Tier 2 | **→ Tier 4 (CRuby wire-format)** | Marshal is ABI-shape serialisation — tied to CRuby version-specific bytes |
| Six rules | **Eight rules** | Added: pure-Ruby canonical (Rule 6), namespace convention (Rule 8); expanded Rule 4 with deviation taxonomy |
| Bun monolith rejected as alternative | **Bun-shape allowed as `everything` feature alias** | Shape and tier are orthogonal; CLI install can default to batteries-included; library install stays opt-in |
| No binary-size budget for outer shapes | **Per-shape budgets locked in** | Resolves Blocker 3 from review |
| Capability-gating deviation "battery owns its own file" | **Closed taxonomy of seven deviation classes (a–g) + three inadmissible classes** | Resolves Major from review: deviation rule was a slippery slope |
| Rule 6 ADR-per-battery as social discipline | **CI-enforced (script checks `_<name>` feature → matching ADR file)** | Consistent with project's panic-budget / RSS-budget ratchet culture |
| `File`/`IO` matrix erased Tier 1 host-fn path | **Tier 1 host-fn mechanism explicit; `_io` battery is Ruby-class veneer on top** | Preserves ADR 0017's set_stdout/register_fn story |
| No namespace convention | **`require "rubyrs/<name>"` for native batteries; pure-Ruby keeps bare names** | Prevents collision with Tier 4 MRI gems; matches Node's `node:` prefix solution |

### What this is not

- **Not a roadmap.** ADR 0019 specifies the boundary, not the
  order. Whether SQLite, HTTP, or `Fiber` ships first is
  release-planning, not architecture.
- **Not a license to skip ADR 0018's phase ordering.** Tier 2 /
  Tier 3 work still happens **after** the Phase 1
  `rubyrs-core` extraction. ADR 0019 doesn't reshape the
  migration; it specifies what gets extracted into
  `rubyrs-language` and `rubyrs-stdlib` when their phases
  arrive. **Phase 0 audit (ADR 0018) needs the following
  additions per ADR 0019**:
  - Inventory `vm/fileops.rs` → Tier 3 `_io` battery (Ruby
    class layer) + Tier 1 host-fn primitives (stay)
  - Tag every module destined for `rubyrs-language` vs
    `rubyrs-stdlib` per the matrix above
  - Re-spec the `stdlib` feature stub at
    `crates/rubyrs/Cargo.toml` to "umbrella over pure-Ruby
    canon only"
  - Record `rubund`'s declared tier (Tier 2 — `rubyrs-language`)
- **Not a permanent capability-gating waiver for native
  batteries.** Rule 4's seven-class taxonomy is closed for
  v0.x; expanding it requires amending this ADR.
- **Not a freeze on language semantics.** Tier 1 still gets
  feature work (the diff_cruby gap-reports continue). ADR 0019
  only constrains *where* outer-tier work lands.

## Consequences

### What gets easier

- **First Tier 3 PR has a home.** `_sqlite`, `_http`, `_s3`
  each have a pre-decided crate layout, a written rule for
  capability-gating with seven enumerated deviation classes,
  a CI-enforced ADR requirement, and a namespace
  convention. The PR is "implement the battery", not
  "argue the boundary."
- **rubund's import shape becomes clear.** It depends on Tier
  2 (`rubyrs-language`). This unblocks rubund's structural
  placement once Phase 1+3 of the workspace migration land.
- **Bun-class story has architectural backing.** "Native
  SQLite shipped in the binary" is a credible claim
  because we have:
  - A named tier slot (Tier 3 native battery)
  - A vendor crate convention
  - A capability deviation taxonomy that accepts SQLite's
    "owned-resource I/O" pattern
  - A `cli-defaults` shape alias that doesn't drag in the
    fat batteries by default, plus an `everything` shape
    alias that does
  - A binary-size budget per shape
- **Per-battery cost stays bounded.** ADR-per-battery (Rule 7)
  + per-battery feature flag + per-shape size budget means
  a SQLite PR can't drag in HTTP design choices or v.v.
- **Embed/wasm targets are protected.** Putting Thread
  (and Ractor) at Tier 3 instead of Tier 2 means an
  embedder building for `wasm32-unknown-unknown` doesn't
  pay for code that physically cannot run there. This
  matters because the WASM cold-start story (~7 ms vs
  CRuby's ~78 ms) is one of the project's load-bearing
  marketing claims.
- **Multiple deployment shapes from the same workspace.**
  `cargo add rubyrs-core` (embed), `cargo install rubyrs`
  (cli-defaults — standard CLI tool), `cargo install
  rubyrs --features everything` (Bun-class) all build
  from one `git clone`. wasmtime model.

### What gets harder

- **More ADRs.** Each Tier 3 battery wants ~50–100 lines of
  its own ADR. Estimate: 4–6 batteries in the v2 timeframe →
  4–6 new ADRs. CI-enforcing the requirement (Rule 7) raises
  the per-PR cost but keeps "stdlib creep" from being
  silent.
- **Cargo feature matrix grows.** Today: `cext`, `regex`,
  `bignum`, `stdlib` (pending). After 0019 + first batteries:
  + `_sqlite`, `_http`, `_s3`, `_thread`, `_io`, plus
  `cli-defaults`, `everything`. The CI matrix moves from
  every-combination (combinatorial, intractable) to a
  **minimum viable matrix of 5 jobs**:

  | Job | Features | Purpose |
  |---|---|---|
  | embed | `--no-default-features` (`rubyrs-core`) | Tier 1 only; size ceiling |
  | default | (whatever `rubyrs-core` defaults to) | Library default |
  | cli-defaults | `--features cli-defaults` | Standard CLI install |
  | everything | `--features everything` | Maximalist build; size ceiling test |
  | wasm | `--target wasm32-unknown-unknown --no-default-features` | bare-WASM verification (ADR 0018 Phase 2) |

  Combinatorial coverage is achieved by reviewer judgment
  per-PR, not by CI matrix size.
- **Pure-Ruby vs Rust-backed parity questions surface.** When
  both `JSON` and `_json_native` exist, the native one is
  required to match the pure one's behaviour. Rule 6 makes
  this an architectural commitment, but the testing cost
  is real — `_json_native`'s CI has to run the pure JSON
  test suite as well.
- **Encoding question now blocking.** The Tier 1 / Tier 2
  encoding split is named as a TBD in the matrix; the
  first Tier 3 battery that produces non-UTF-8 strings
  (`_csv` reading Latin-1) forces resolution. Possibly a
  short follow-up ADR 0020.

### What we explicitly accept trading away

- **The "pure-Ruby stdlib" purist position.** Tier 3 ships
  Rust code (the batteries). The alternative — Bun-class
  native batteries living outside the tier system as
  ad-hoc crates — costs more architectural coherence than
  it saves. We accept the purist criticism.
- **Symmetric implementation across batteries.** Each
  battery chooses its own crate (rusqlite vs sqlx;
  reqwest vs ureq vs hyper) and its own surface. We don't
  enforce a `trait Backend` abstraction across batteries —
  premature abstraction in a domain (network / disk /
  native bindings) where each crate has its own idiom.
- **A `cargo install rubyrs` that opens "small."** Bun
  ships ~80 MB by default; we ship `cli-defaults` (~25 MB
  budget). Users who want Bun's all-in shape add
  `--features everything`. This is a marketing trade-off:
  we look slimmer at first install but require an explicit
  flag to demonstrate the Bun-class story. Acceptable.

## Alternatives considered

1. **"Tier 3 = pure Ruby only" boundary.** Clean and easy to
   explain. Boxes out the Bun-class differentiation move.
   Forces Rust-backed batteries into either Tier 2 (wrong —
   they don't change VM) or out-of-tier crates (loses the
   workspace coherence ADR 0015 was built to preserve).

2. **"Tier 3 = pure Ruby; new Tier 5 = native batteries"
   five-tier model.** Splits the difference syntactically
   but adds a ring to ADR 0015's four-ring picture. Pays
   conceptual debt to dodge a paragraph of explanation.
   Rejected on legibility grounds.

3. **Bun-shape rejected entirely** (v1 position).
   v1 of this ADR rejected "one binary all batteries" on
   grounds that it kills the 5 MB embed pitch. The review
   pointed out that shape and tier are orthogonal — the
   embed crate (`rubyrs-core`) can stay 5 MB while the CLI
   facade defaults to `cli-defaults` and optionally goes to
   `everything`. v2 (this revision) reverses the rejection:
   **Bun shape is allowed as a feature alias**, and serves
   the Bun-class marketing story rather than working
   against it.

4. **"One Tier 3 feature drags in everything."** Convenient
   at build time but loses per-battery cost accounting.
   `cargo install rubyrs --features stdlib` would suddenly
   carry `reqwest` (8 MB), `aws-sdk-s3` (15 MB+),
   `rusqlite` (4 MB) for an embedder who wanted CSV. Kills
   the "≤ 25 MB cli-defaults" budget.

5. **"No native batteries; bind via host fns only."** Forces
   embedders to wire up SQLite themselves via
   `register_fn_v2`. Architecturally pure (capability
   gating 100% intact) but loses the Bun-class lever. The
   compromise in Rule 4 (closed seven-class deviation
   taxonomy, ADR-per-battery) is the pragmatic middle.

6. **"Pure-Ruby native shim layer in Tier 2."** Have the
   runtime expose a fixed set of primitives (e.g.
   `Native.sqlite_open(path)`) and write the Ruby-side
   `SQLite3::Database` class in Tier 3 pure Ruby. Two-layer
   discipline; doubles the per-battery cost. This is
   exactly what CRuby does (C ext + Ruby wrapper) and what
   makes its stdlib boundary perpetually fuzzy. Rejected.

7. **Putting Thread / Mutex in Tier 2** (v1 position).
   Reasoning was "implementing it requires VM changes,
   therefore Tier 2." Review identified this as confusing
   necessary and sufficient conditions: Fiber also requires
   VM changes but is OS-capability-free. OS threads bring
   OS capability + GC complexity + wasm exclusion, all of
   which are Tier 3 / opt-in considerations. v2 moves
   Thread to Tier 3 `_thread` battery (deviation class `g`).

## Revision log

- **2026-05-27 — v2 (this revision).** Major rewrite after
  three parallel agent reviews flagged 3 blockers, 8 majors,
  and 4 minors against v1. Resolutions:
  - Blocker 1 (Thread vs ADR 0017): moved Thread/Mutex/
    Queue to Tier 3 `_thread` battery
  - Blocker 2 (ObjectSpace vs ADR 0017): split into Tier 2
    bounded form + Tier 4 full surface
  - Blocker 3 (no size budget): added per-shape budget
    table
  - Majors: closed deviation taxonomy (Rule 4); pure-Ruby
    canonical (Rule 6); namespace convention (Rule 8);
    mechanical Rule 7 enforcement; Phase 0 audit
    additions enumerated; File/IO host-fn path preserved
  - Plus: Bun-shape now allowed as `everything` feature
    alias; CLI / library default distinction explicit
- **2026-05-27 — v1 (commit `d53b044a`, kept in git
  history).** Initial draft proposing implementation-locus
  axis and six rules.

## Related

- [ADR 0015 — Concentric architecture](0015-concentric-architecture.md)
  — the four-tier shape this ADR refines. Rule 2 ("opt-in,
  not opt-out") gets a CLI-vs-library carve-out in Part C.
- [ADR 0017 — Tier-1 boundary specification](0017-tier1-boundary.md)
  — the inner-ring spec; ADR 0019 mirrors the exercise one
  ring out. v2's Thread and ObjectSpace placements align
  with ADR 0017's existing rows.
- [ADR 0018 — Workspace migration plan](0018-workspace-migration.md)
  — the phased path to land the multi-crate split. ADR 0019
  shapes what Phase 3 (`rubyrs-language` extraction) and
  Phase 4 (`rubyrs-stdlib` extraction) carry. **Phase 0
  audit must be extended per "What this is not" above.**
- [ADR 0007 — Host embedding API](0007-host-embedding-api.md)
  — the `Runtime` / `Config` / `register_fn` surface that
  Tier 3 batteries route through when they respect
  capability gating (Rule 4 non-deviation path).
- [ADR 0009 — cext panic policy](0009-cext-panic-policy.md)
  — the cext ABI used by msgpack / flori_json / bcrypt is
  itself a Tier 4 mechanism, but the policy applies to any
  Tier 3 battery that opts into cext-style native code.
