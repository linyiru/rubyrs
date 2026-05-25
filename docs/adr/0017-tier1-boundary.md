# 0017: Tier-1 boundary specification

## Status

Proposed (2026-05).

## Context

[ADR 0015](0015-concentric-architecture.md) committed rubyrs to a
concentric-tier shape — `core` (Tier 1) inside, `language`,
`stdlib`, `mri-compat` outward. It did not specify what,
concretely, belongs inside the tier-1 boundary. Without that
spec, every PR that introduces a new feature reopens the same
debate ("is this core enough?"), and outer tiers leak inward by
accretion — which is the exact failure mode ADR 0015's "core is
the constitution" rule was meant to prevent.

PR #73 (the cext-feature PoC, merged) empirically confirmed that
tier gating works mechanically but is *not* "5-minute clean" per
boundary; ~13 cfg sites across 6 files for one tier. That's
affordable as long as we know which tier each feature targets
ahead of time. Without ADR 0017, we don't.

We also have new empirical data on what mature embeddable
scripting languages put in their tier 1 — verified by reading
Lua 5.4 source, mruby `mrbgems.md`, rhai 1.25 `Cargo.toml`, rune
0.14 workspace structure. Their convergent design choices are
strong evidence for what tier 1 should look like.

## Decision

Adopt the following Tier-1 inclusion rules. Anything that meets
all four is in. Anything that fails any one is outside.

### The four Tier-1 rules

1. **Deterministic from script inputs alone.**
   Same script text + same `Config` + same host-registered fns →
   identical output, byte-for-byte. No wall-clock time, no
   process ID, no environment variable reads, no randomised hash
   iteration order. Anything non-deterministic enters through a
   host-registered fn or sits in an outer tier.

2. **No syscall, no I/O, no process control.**
   File, Dir, IO, Net::HTTP, Socket, Kernel#system, backtick,
   exec, spawn — all of these belong outside Tier 1. They reach
   the script through capability-gated host fns or through Tier
   3's `stdlib` feature, which embedders explicitly opt into. The
   default `cargo install rubyrs` shape never gives a script the
   ability to touch the OS without the embedder saying so.

3. **No regex.**
   `/pattern/` literals and the Regexp class move to Tier 2 (a
   future `regex` Cargo feature). The full Rust `regex` crate is
   ~300 KB compiled — material relative to a target "tier-1
   embed binary under 4 MB" — and a ReDoS attack vector that is
   not appropriate for a DSL-host runtime by default. The
   parallel: Lua / Wren / rhai / rune / Starlark all decided the
   same way, for the same reasons. Embedders who need regex
   either enable the feature or register a host fn.

4. **No OS threads, no shared mutable globals across executions.**
   `Thread`, `Mutex`, `Queue` belong outside Tier 1. `Fiber` is
   on the boundary — every embeddable scripting language (Lua
   coroutines, mruby Fiber, Wren fiber, rhai single-threaded,
   rune single-threaded core) treats cooperative concurrency as
   the only primitive a script gets. We follow that default:
   Tier 1 ships single-threaded with Fiber TBD per PR.

### What's in Tier 1

| Component | Why it satisfies the four rules |
|-----------|--------------------------------|
| Prism parser + AST + compiler + VM + GC | Deterministic, no syscalls, no regex, single-threaded. |
| `Value`: `Int` / `Float` / `Bool` / `Nil` / `String` / `Symbol` | Pure data; no IO. |
| `Array` / `Hash` / `Range` | Pure data; Hash iteration order must be deterministic (rule 1). |
| `Class` / `Module` / method dispatch | Pure dispatch; no metaprogramming hole that touches the OS. |
| `Block` / `Proc` / `Lambda` (incl. `yield`) | Cooperative; no threading. |
| Exception (`raise` / `rescue` / `ensure` / `retry`) | Pure control flow. |
| String operations **without regex** (`split` literal, `slice`, `sub` literal, `gsub` literal, etc.) | Rule 3. |
| Numeric ops + basic Math (`abs`, `sqrt`, `pow`, integer arithmetic) | Pure. |
| Resource caps via `Config` (`fuel`, `max_heap_objects`, `max_frames`, future: `max_string_size`, `max_array_size`, `max_expr_depths`) | Rule 4 enforcement + per-script bound. The full list is informed by rhai's safety API matrix (see "Related: prior art" below). |
| Embed API (`Runtime`, `Config`, `register_fn`, `register_fn_v2`, `HostCtx`, `set_stdout`) | Capability injection point — the only way OS-flavored capability reaches a script. |

### What's explicitly OUT of Tier 1 (non-goals)

This list is the deliberate spec — not a TODO. Each item, when
asked for, gets an answer: "that's tier N" or "that's not in
scope for rubyrs."

| Component | Tier | Rationale |
|-----------|------|-----------|
| `Regexp`, `/pattern/` literals | 2 (`regex` feature) | Rule 3. |
| `Fiber`, `Enumerator` | 2 (`language` feature) | Rule 4; deferred until a real use case. |
| `Thread`, `Mutex`, `Queue`, `ConditionVariable` | 3 or never | Rule 4. OS threads under a single VM violate the model every embed scripting language has adopted. |
| `File`, `Dir`, `IO`, `Pathname`, `Tempfile` | 3 (`stdlib` feature) | Rule 2. |
| `Net::HTTP`, `Socket`, `OpenSSL`, `URI` | 3 | Rule 2. |
| `Kernel#system`, `` ` ``, `exec`, `spawn`, `Process.*` | 3 or never | Rule 2; capability-gated even when present. |
| `Marshal`, `Psych` (YAML), serialization with state | 3 | Mostly rule 1 (non-deterministic on object id), partly rule 2. |
| `ObjectSpace`, `GC.start`-style reflection | 4 (`mri-compat`) | Violates rule 4 (introspects shared global state); needed only for CRuby-shape parity. |
| `Time` (wall-clock) | 2 with capability injection from host | Rule 1; a `Time.now` host fn is the supported way. |
| `Random`, `SecureRandom` | 2 with seeded mode in tier 1 | Rule 1; deterministic seeded `Random.new(seed)` is fine, system-entropy `Random.new` belongs out. |
| `ENV[…]` reading host-process env vars | 2 (host-injected map only) | Rule 1+2. Direct host-process env reads are out (non-deterministic + capability leak); the supported Tier-1 shape is `Config::env`-injected map exposed under the `ENV` name (TBD API). |
| C extension ABI (`require 'foo.so'`) | 4 (`mri-compat`, **already gated** per PR #73) | Whole-tier-4 surface. |
| Bundler / RubyGems / Gemfile resolution | 4 (out-of-tree; this is `rubund`'s job) | Not interpreter scope. |
| Rails, ActiveRecord, ActionPack | 4 (multi-year bet) | Roadmap-level; not a tier-1 commitment. |

## Consequences

### What gets easier

- **PR-level decision: which tier?** Every new feature now has a
  decision tree. "Does it need a syscall? → not Tier 1." Removes
  ~90% of the per-PR scoping debate.
- **Honest sales pitch.** The README's "today: DSL host;
  tomorrow: Rails-capable bet" framing has architectural backing:
  Tier 1 is the today-shippable surface, the outer tiers stay
  honest about their bet-vs-promise status.
- **Sandbox story is consistent.** Hosts running untrusted
  scripts get the documented guarantee: build with
  `--no-default-features` (or with only the `core` tier), and
  every OS-touching capability is in their hands.
- **Reviewer + contributor checklist.** "Is this PR adding a
  syscall to Tier 1?" becomes a literal text-search check on the
  cfg gates.

### What gets harder

- **`Time.now`, `ENV` need host-side patterns.** Both are
  routinely used in real Ruby code. The supported tier-1 shape is
  "host registers `now_ms` / `getenv` host fns and binds them to
  `Time.now` / `ENV[]` in script-visible names." Until we ship a
  Tier-1 cookbook for this, contributors will have to find their
  own way.
- **`Random` requires a seeded vs. unseeded split** even at the
  type level. CRuby exposes `Random.new` (system entropy by
  default); we'd want script-level `Random.new(seed)` only in
  Tier 1, and document the divergence.
- **`String#match`, `String#=~` removal** when regex moves to
  Tier 2 will surface as parse errors in real-world Ruby
  snippets. The diagnostic needs to point at the `regex` feature
  rather than say "not supported". This is the same UX we landed
  for `require` without `cext` in
  [PR #75](https://github.com/linyiru/rubyrs/pull/75).

### What we explicitly accept trading away

- **CRuby drop-in compatibility.** Existing Ruby scripts that
  freely use Time, ENV, File, Net::HTTP at top level will not
  work on a default `cargo install rubyrs`. Outer tiers exist
  for those scripts; the default install is the embed/DSL niche.
- **One-line "just works" Rails attempt.** A Rails app cannot
  load against Tier 1. The roadmap's Rails bet sits at Tier 4
  for a reason. The honest framing matters more than the
  short-term ergonomics.
- **Feature parity with mruby's "near full Ruby" default
  gembox.** mruby ships a `full-core` gembox that pulls
  everything in; we ship the opposite default. We're explicit
  about the choice.

## Alternatives considered

1. **No spec.** Leave each PR to argue tier-ness from first
   principles. This is what we have today; this ADR exists
   because it doesn't scale beyond ~5 contributors.

2. **Define Tier 1 maximally — "everything that's not a syscall is
   Tier 1."** This is mruby's `full-core` gembox approach.
   Rejected: optimises for "script feels like CRuby today" at
   the cost of binary size, sandbox guarantees, and
   determinism. Loses the embed/DSL niche we're winning at.

3. **Define Tier 1 minimally — "only the parser, VM, basic
   types."** This is Wren's posture. Rejected: too thin for the
   Brewfile/Gemfile use case rubyrs ships today (which needs
   Block, Proc, Hash, Array, simple String ops). A tier 1 that
   can't run today's working examples isn't a useful spec.

4. **Per-target tier defaults** (e.g. wasi defaults to tier 1
   only). Rejected pragmatically: Cargo does not support
   target-specific default features, and the build.rs panic
   approach (used for `cext` + wasi in
   [PR #75](https://github.com/linyiru/rubyrs/pull/75)) does not
   generalise to "the whole tier matrix." Targets opt into
   tiers explicitly via `--features`.

## Related: prior art (empirical, verified 2026-05)

This spec was written after empirically verifying tier-1 design
choices in four mature embeddable scripting languages. The
convergence is striking — none of them ships regex, OS threads,
or unsanitised IO in their default tier-1 surface.

| Language | Tier 1 stdlib | Regex | OS threads | Resource caps | Workspace |
|----------|--------------|-------|------------|---------------|-----------|
| **Lua 5.4** | 10 modules (table/string/math/coroutine/package/utf8/io/os/debug), all individually strippable via `luaL_requiref` rather than `luaL_openlibs` | None; "Lua patterns" (<500 lines vs POSIX 4000+) per the official PIL | None; only coroutines | `lua_sethook(LUA_MASKCOUNT, n)` for instruction limits + custom `lua_Alloc` for memory caps | Single C file pair |
| **mruby 3.x** | mrbgems — base + default gembox + full-core gembox, fully configurable in `build_config.rb`. Regex not in default gembox; needs `mruby-onig-regexp` | Optional via mrbgem | None; Fiber only | `code_fetch_hook` via `MRB_ENABLE_DEBUG_HOOK` build flag; `MRB_HEAP_PAGE_SIZE` for memory | Single repo, gem-modular at build time |
| **rhai 1.25** | Cargo `[features]` with negation flags: `no_float`, `no_index`, `no_object`, `no_time`, `no_function`, `no_closure`, `no_module`, `no_custom_syntax`. Plus orthogonal `sync` for `Send+Sync`, `unchecked` to remove all safety checks | None in core | Single-threaded by default; `sync` feature for cross-thread Engine | **The most complete matrix we found**: `set_max_operations`, `on_progress`, `set_max_call_levels`, `set_max_array_size`, `set_max_map_size`, `set_max_string_size`, `set_max_modules`, `set_max_expr_depths` | Single main crate + `codegen` for proc-macros |
| **rune 0.14** | Multi-crate workspace: `rune-core`, `rune-alloc`, `rune-macros`, `rune-modules`, `rune-cli`. Notable: opposing `capture-io` / `disable-io` features make IO an explicit capability | None | Single-threaded stack-based core; async runtime injected by host (not core) | `rune::runtime::budget::Budget` for instruction limits | Multi-crate workspace ★ |

Two takeaways shape rubyrs's spec above:

- **rune's `capture-io` / `disable-io` feature pair is the cleanest
  "IO as capability" pattern** in the Rust embeddable-language
  ecosystem. ADR 0017 adopts the same posture: the default `core`
  build has no IO; reaching the OS requires either a host fn (the
  Tier-1 supported path) or opting into the `stdlib` feature
  (Tier 3).

- **rhai's safety API matrix is more complete than rubyrs's
  `Config` today.** The current `Config` exposes `fuel`,
  `max_heap_objects`, `max_frames`. rhai adds
  `max_string_size`, `max_array_size`, `max_map_size`,
  `max_expr_depths` — each a real sandbox attack surface we
  haven't covered. Listed as roadmap items above; not blocking
  this ADR.

**Notably absent**: none of these four projects document an
explicit *non-goals* list. mruby has `doc/limitations.md` which
is the closest. The empty space in prior art means this ADR's
"What's explicitly OUT" table is itself a contribution — for
contributors who want to know what shape rubyrs is *not* trying
to become.

## Related ADRs

- [ADR 0007 — Host embedding API](0007-host-embedding-api.md) —
  the Tier 1 capability-injection surface this spec depends on.
- [ADR 0008 — Resource caps for untrusted scripts](0008-resource-caps-for-untrusted-scripts.md) —
  the `Config::fuel` / `max_heap_objects` / `max_frames`
  facilities that satisfy rule 4 enforcement.
- [ADR 0015 — Concentric architecture via tiered Cargo features](0015-concentric-architecture.md) —
  the four-tier shape this ADR populates with concrete content.
- [PR #73](https://github.com/linyiru/rubyrs/pull/73) — empirical
  validation that one tier boundary (`cext`) is achievable
  through Cargo features + cfg gates.
- [PR #75](https://github.com/linyiru/rubyrs/pull/75) — post-PoC
  hardening of the `cext` feature gate; the place this ADR
  cites for the diagnostic-message UX (`require` without the
  feature) and for the `build.rs` panic precedent (rejecting
  the wasi+cext combination loudly).
