# 0026: Omakase blessed-gem menu — curated reimplementations, not gem hosting

## Status

**Proposed v2 (2026-05-31).** v2 incorporates a 4-agent independent
review that surfaced 10 issues with v1 (originally promoted from
`poc/sinatra/STRATEGY.md` after PR #315 landed). The v2 delta vs v1:

1. **Version policy** — every menu item pins an upstream version; major
   bumps need their own follow-up. Parity % is measured against the
   upstream gem's own test suite, not a hand-picked behaviour subset.
2. **Anti-pattern named**: blessed reimpls MUST NOT engine-branch on
   `RUBY_ENGINE` / `RUBYRS`. The discriminator is a *library-author*
   tool for adapter shims, not for in-tree blessed code.
3. **Menu tier reclassification**: ActiveSupport-lite slice moved from
   "mostly Tier-1" to **pure-Ruby Tier-3 canon**
   (`_pure_activesupport_lite`) per ADR 0019 §"Pure-Ruby canon"
   classification. ERB-lite carries a hard dependency on `_full_eval`
   (Tier-2) and is re-costed.
4. **Parity-by-test honesty**: v1 claimed it "generalises" the existing
   `diff_cruby.rs` harness. v2 acknowledges that's a misnomer —
   framework-fixture parity (boot server + replay route matrix + diff)
   is new infrastructure (M27 below), not a generalisation.
5. **"Open the harness"**: the parity tool gets a public-CLI sibling
   (`rubyrs-parity`) so any gem author can self-certify against rubyrs.
   The menu stops being the only on-ramp.
6. **Exit clause**: menu items sunset when upstream becomes loadable
   at parity (real gem-on-WASM with full ABI). Prevents the menu from
   becoming a parallel-maintenance liability if the ecosystem catches up.
7. **Transitive-deps closure policy**: the menu is closed *under
   transitive `require` deps* — every gem an on-menu item pulls is
   either on the menu, stubbed with a documented behaviour limitation,
   or causes `require "<on_menu_gem>"` to fail at load time with a
   clear "depends on X — off-menu" message.
8. **ADR 0019 Rule 6 carve-out**: framework batteries (Sinatra,
   ActiveSupport, etc.) are a documented exception to "pure-Ruby
   canonical, native is accelerator." The upstream gem IS the canon;
   the rubyrs vendored reimpl is parity-tested *against the gem*, not
   maintained as a co-equal pure-Ruby reference. Otherwise Rule 6 is
   unimplementable for framework-shaped batteries.
9. **Enforcement link**: parity-by-test and `require "rubyrs/<name>"`
   resolution both delegate to ADR 0019 Rule 8's registration helper
   (`Runtime::register_native_battery`) and its `grep` audit.
10. **Critical path is M27** (block-parameter AST + parity-harness v2),
    not the headline menu rollout. M27 unblocks 3+ menu items with one
    landing and is the deferred work the v1 "highest-leverage" list
    failed to sequence.

Sources: this repo's ADRs (0015, 0017, 0019, 0022, 0023), `docs/ROADMAP.md`
gapscan data, PoC results in `poc/sinatra/`, four parallel review reports
(2026-05-31) covering architecture, risk, competitive, and execution-path
angles.

## Context

The recurring question "can rubyrs run gems?" has two pole answers, both
wrong:

- **Pole A: Bundler + every gem (Tier 4).** Resolves arbitrary
  `Gemfile`s, compiles C extensions. Artichoke died trying. ADR 0015
  § "Why not Artichoke?" cites this. Multi-year, single-maintainer-killing,
  C-ext-ABI bet.
- **Pole B: Pure embedding subset forever (mruby pole).** Permanently
  forecloses the web-framework conversation the embedding brief is asking
  about.

ADR 0015 named "Tier 2 (`language`) — Sinatra/Rack capable" as the
deliberate position between those poles. ADR 0019's implementation-locus
axis + native batteries + `require "rubyrs/<name>"` Rule 8 is the
populating mechanism. The Sinatra PoC (PR #315) is the existence proof:
one byte-identical `app.rb` runs on CRuby + the real `sinatra` gem AND
on rubyrs + a vendored micro-Sinatra, byte-identical responses across
16 routes.

## Decision

Adopt **"omakase = a parity-tested menu of blessed reimplementations"**
as the official answer to "can rubyrs run gems?":

1. **Don't host gems. Bless a menu.** A small, curated set of
   rubyrs-native reimplementations of high-value gems' public surface
   — vendored, integration-tested, namespaced, chosen by the
   maintainers. PR #315's `sinatra_lite.rb` is the prototype.

2. **Don't invent architecture, populate the existing tiers.** ADR
   0015/0019 already specify the surface (Tier 2 + native batteries +
   `require "rubyrs/<name>"`).

3. **Parity, not coverage, is the quality bar.** Every blessed item
   passes a framework-fixture diff against the **real gem** at a
   pinned version. CI gate.

4. **Sinatra now; Rails explicitly conceded.** Rails is Tier-4 (the
   pole we don't pursue). rubyrs's positioned market is *everything
   Sinatra-shaped that wants to be embedded in a Rust binary* — CDN
   edge handlers, game-mod scripting, plugin systems, in-process
   admin DSLs. Stop hedging on Rails.

5. **The parity harness is also a product.** A public `rubyrs-parity`
   CLI lets any gem author self-certify their own gem against rubyrs.
   The menu is the curated set; the harness is the gradient.

## Version policy

Each menu item carries an explicit version pin. The published menu is
a row of `(name, upstream_version, parity_pct, ci_url)`.

- **Initial version** stated in the per-menu-item ADR (Rule 7).
- **Minor / patch bumps** track upstream within ~3 months; bump is a
  single chore PR with an updated parity report. No new ADR.
- **Major version bumps** require a follow-up ADR justifying the
  re-target (the upstream API surface changed; the parity matrix is
  effectively rewritten). Examples: Rack 2→3 reshaped the body
  protocol; Sinatra 1→2→4 changed how `helpers` resolve.
- **Parity %** is measured as `(passing tests in upstream's own test
  suite when run against our reimpl) / (total tests in that suite)`,
  not a hand-picked behaviour list. This grounds the metric in an
  authoritative denominator that grows when upstream adds features.
- **Sunset**: see §"Exit clause" below.

## Anti-pattern: no engine-branching inside blessed reimpls

The headline promise is "same code runs on both runtimes" (PoC `app.rb`
is byte-identical). To keep that honest, blessed reimpls
(`rubyrs/sinatra`, etc.) MUST NOT contain `if RUBY_ENGINE == "rubyrs"`
or any equivalent discriminator. Engine-branching is a *library-author*
tool for **external** adapter shims — `poc/sinatra/sinatra_compat.rb` is
the canonical example: a 25-line file that picks who provides
`Sinatra::Base` and stays out of the parity loop. Inside the blessed
reimpl, behaviour must be parity-tested end-to-end; an engine-branch is
an unobservable escape hatch the parity matrix can't catch (CRuby path
never runs on rubyrs CI).

GAP #4 (a `RUBYRS` discriminator) is **for the shim layer only** — not
a license to skip parity tests on engine-branched code paths inside
blessed reimpls.

## The omakase menu — opinionated priority order

| # | Menu item | Why | Mechanism / tier | Readiness |
|---|---|---|---|---|
| 0 | **Rack contract** (`[status,headers,body]`, env) | everything web sits on it | `_http_server` battery (Tier 3) | **shipped** (ADR 0022) |
| 1 | **Sinatra** (modular DSL, routing, params, filters, `halt`, templates-lite) | smallest real framework; the on-ramp | vendored `rubyrs/sinatra` (framework battery on `_http_server`) | **PoC works** post-PR #315; templating GAP #13 still pending; M27 unblocks plugin code |
| 2 | **JSON** (parse/generate) | every API needs it; `as_json` | pure-Ruby canon (Rule 6) + `_json_native` accel | **fully shipped (2026-06).** Pure canon `src/stdlib_vendor/json.rb` covers parse/parse!/generate/pretty_generate/dump/load/unparse/pretty_unparse + `JSON[]` shortcut, `to_json(state)` + `as_json` mixins on basic types incl. `Object` fall-through, `JSON::State`, `JSON::JSONError`/`ParserError`/`GeneratorError`/`NestingError` hierarchy, `symbolize_names:` / `allow_nan:` / `max_nesting:` options, NaN/Infinity literal parsing; deterministic subset byte-identical to CRuby stdlib. `_json_native` accelerator (serde_json with `preserve_order`) `src/json_native.rs` wires `__rubyrs_json_native_parse` / `_generate` host fns; canon auto-detects via `defined?(...)` and routes the deterministic-default path through Rust. Bench: parse **0.62× Oj** (rubyrs FASTER than the fastest Ruby JSON gem via streaming serde Visitor), generate 1.11× Oj (byte-buffer + ASCII fast-path), pure canon ≈ 160–200× CRuby stdlib (`bench/json_bench_results.md`). |
| 3 | **ActiveSupport-lite core-ext slice** (`blank?`, `present?`, `Hash#deep_*`, `String#camelize`) | the connective tissue every Ruby web app assumes | **`_pure_activesupport_lite` (Tier-3 pure-Ruby canon)** vendored | **Tier A+B+C shipped (2026-06)** via `src/stdlib_vendor/active_support_lite.rb` — `blank?`/`present?`/`presence` family on Object/NilClass/TrueClass/FalseClass/String/Array/Hash/Numeric, `Array#second/third/fourth/fifth/in_groups_of`, `Object#try/#in?`, Hash `symbolize_keys`/`stringify_keys` + `deep_*` + `deep_merge`, String `squish/camelize/underscore/dasherize/titleize/humanize/truncate/blank?`. Deterministic subset byte-identical to `active_support/all` on CRuby 8.x; pinned by `as_lite_canon` framework-parity fixture. **Tier D deferred** (Numeric duration helpers + Time.current + Time.zone — chain through ActiveSupport::Duration + tzinfo, see `poc/as_lite/GAPS.md` §Tier D). |
| 4 | **A data layer on `_sqlite`** (a Sequel-lite / ROM-lite query DSL, *not* ActiveRecord) | "Sinatra + a DB" is the first real app | `_sqlite` battery (Tier 3) + pure-Ruby DSL | **Phases 1–3.1 shipped (2026-06)**: `poc/sqlite/FINDINGS.md` discovery + `docs/adr/0027-battery-sqlite.md` lock the design (rusqlite + bundled libsqlite3, single-conn `SQLite3::Database`, block-form transactions, LRU(100) prepared-statement cache, 25-class exception hierarchy, `Config::sqlite_allow_paths` sandbox, `cli-defaults`/`everything` aggregates). Phase 3 = battery PoC + `SQLite3::Database` Ruby surface; Phase 3.1 = `SQLite3::Database#prepare → SQLite3::Statement` + `bench/sqlite_bench.rb` showing rubyrs ahead of CRuby by 20–37 % on 3 of 4 workloads (4th within ~10 % noise — see `bench/sqlite_bench_results.md`). **Phases 5–6 (Sequel-lite Dataset DSL + parity fixture) deferred** at the Phase 3.1 close — the raw battery is already competitive and shipping the chainable Dataset speculatively risks a mid-state "looks like Sequel but isn't". Re-opens when a concrete consumer drives the DSL shape; see ADR 0027 §Migration plan for the deferral note. |
| 5 | **ERB-lite / Tilt-lite templating** | views | **depends on `_full_eval` (Tier-2, ADR 0019)** | substantially more work than table position suggests; not a weekend project |
| — | **Rails / ActiveRecord** | the dream | Tier 4 bet | **conceded**, not just deferred |

The line between #5 and Rails is the honest edge of the map. Items 4
and 5 carry dependencies the v1 table elided (#4 on harness v2; #5 on
`_full_eval`); they're left in the menu but with explicit cost notes.

## The compatibility contract

The brief's real requirement — *"the same code runs on CRuby and
rubyrs"* — is the governing constraint:

1. **Parity-by-test (M27).** v1 framed this as "generalising
   `tests/diff_cruby.rs`." v2 acknowledges that's wrong: the existing
   harness runs script-file fixtures via `--disable=gems`, not a real
   gem-loaded CRuby process. The framework-fixture harness
   (`tests/diff_framework/<item>/{app.rb, fixtures.yml, expect/}`) is
   new infrastructure scheduled for **M27** (see §"Critical path").
   It generalises `poc/sinatra/verify.sh` (bash + curl + sed-normalised
   diff, 126 LOC) into a Rust framework that spawns both runtimes,
   replays a declarative route/scenario matrix, byte-diffs output, and
   exposes a stateful-lifecycle hook for stateful items (the SQLite
   menu item needs schema seed + post-scenario `.dump` diff).

2. **Honest resolution.** `require "sinatra"` resolves to the blessed
   reimpl only when the real gem isn't loadable; `require
   "rubyrs/sinatra"` always pins the built-in. Both routes go through
   `Runtime::register_native_battery` per ADR 0019 Rule 8 — the same
   mechanism that prevents two batteries from claiming the same bare
   name. No silent shadowing; off-menu → clean `LoadError` with the
   gem name and the menu URL.

3. **Published menu + parity %.** A single page on the project site
   (machine-generated from `crates/rubyrs/menu_state.json`) lists
   every menu item, its pinned upstream version, the parity % against
   that upstream's test suite, and the CI workflow URL. Updated
   per-merge.

## Open the harness

The strongest move in v2: the parity harness is itself a product. Ship
`rubyrs-parity` as a standalone CLI (a thin wrapper around the same
Rust harness `diff_framework/` uses internally) so any gem author can
run their gem's test suite under rubyrs and produce a parity report.
Output: identical to the published-menu format above.

Effect:
- Externalises the menu maintenance that §Consequences correctly
  flags as a burden — gem authors who care self-certify.
- Turns "rubyrs supports my gem" from a gate-kept claim ("submit a
  PR to the menu") into a gradient ("here's my parity %").
- Pre-empts the political risk where an upstream author objects to a
  shadow reimpl: the upstream can run the harness against their own
  code without engaging the rubyrs project at all.
- Zero marginal cost — the harness already exists for the menu.

The closed menu and the public harness coexist: the menu is what
*rubyrs maintainers* commit to. The harness is what *anyone else* can
use.

## Transitive-deps closure policy

The menu is closed under transitive `require` deps. For every blessed
item N:

1. Every gem N pulls via `require` (or its lazy equivalents) is either:
   - **On the menu** (the most common case for foundational gems —
     Sinatra→Rack→Mustermann→Tilt all need to be on the menu together,
     not separately);
   - **Stubbed** with a single-page behaviour-limitation note
     (e.g. `rack-protection`: "we provide the API surface but the
     CSRF/clickjacking/XSS shields are no-op stubs — embed-side
     responsibility");
   - **Refused at load** — `require "<N>"` raises `LoadError` with
     "rubyrs/<N> depends on <off-menu-dep>; off-menu" and a link to
     the menu page.

2. Refusal is the default. Stubbing requires per-stub justification
   in the item's ADR (Rule 7). Silent allow-through is forbidden:
   the user-visible failure mode (route-handler `LoadError` deep in
   the request) is worse than the load-time refusal.

3. The harness checks closure: a `cargo test -p rubyrs-menu-audit`
   run loads each menu item under rubyrs, walks its `require` graph,
   and asserts every reachable name is in one of the three categories.

## Critical path: M27

> **Status (2026-06): M27 shipped.** Batches A1/A2/A3/A4 (block params,
> middle-splat, define_method block capture), B1/B2 (`#call`-able Rack
> app + `RUBYRS` sentinel), C1 (`Hash#to_s`), and D (parity harness +
> `hello_smoke` + `sinatra_hello` fixtures + `framework-parity` CI job)
> are all on master. See commits `bbafff6c` … `c4e0511f`. The harness
> lives at `crates/rubyrs/tests/diff_framework/`; menu item 1 (Sinatra)
> now has byte-diff parity gated on every PR.
>
> **Two known harness deltas absorbed via manifest normalize rules** —
> recording them here so future menu fixtures don't relearn:
>
> 1. **`require "<gem>"` side effects.** classic-style Sinatra
>    autostarts a Puma server at port 4567 from `require "sinatra"`'s
>    at_exit hook; the harness's gem-availability probe would clobber
>    its own free-port pick. Fixtures **must** declare `required_gems`
>    using the no-autostart entrypoint (`sinatra/base`, not `sinatra`).
> 2. **Header ordering between Sinatra+WEBrick and rubyrs's
>    `_http_server` battery.** WEBrick emits `Location, Content-Type`;
>    rubyrs emits the reverse. `run_matrix` collects filtered headers
>    into a `BTreeMap` so the transcript is canonical (sorted) before
>    byte-diff. Additionally, WEBrick appends `;charset=utf-8` to
>    default Content-Type; rubyrs emits bare media type. Both are
>    valid per RFC 7231 §3.1.1.5 — fixtures normalize via a regex rule.

The next milestone is **M27 — Block-parameter AST family + parity
harness v2**:

1. `BlockParameterNode` / `RestParameterNode` / `SplatNode` (gapscan
   #1 missing AST family across Sinatra plugins, Tilt, dry-struct).
   Pure Tier-1 work, no Tier-2 boundary scope. **One landing unblocks
   menu items 1 (third-party Sinatra plugins), 4 (Sequel-lite DSL
   builders), and 5 (Tilt source).**
2. GAP #4 (`RUBY_ENGINE` / `RUBYRS` discriminator) and GAP #3
   (`#call`-able Rack app instead of bare lambda) — bundled in, ~1 day
   each.
3. `tests/diff_framework/` Rust framework (described in
   §"Compatibility contract" above).

Rationale: M27 is the single landing that moves gapscan's #1 AST
blocker across three menu items AND ships the parity infrastructure
the §"Compatibility contract" already promises. GAP #13
(`_full_eval` for templating) is **deliberately deferred** — it's
quarter-scale Tier-2 work and should be driven by the actual demand
profile from menu items 1–4 shipping, not pre-committed.

## Exit clause

Menu items sunset when upstream becomes loadable at parity. Specifically:

- If a real-Ruby ABI ships on WASM (ruby.wasm 3.x + real gem ABI, or
  Artichoke 2.0 with C-ext compat, or any successor) such that
  `gem install <upstream>` + `require "<upstream>"` works in a
  rubyrs-compatible host AND achieves ≥95% parity on the upstream's
  own test suite, **the blessed reimpl is deprecated**. The blessed
  reimpl receives a one-release deprecation window (`require "rubyrs/<N>"`
  emits `warn` pointing at the upstream-via-real-gem path), then is
  removed.

- Without this clause, the menu becomes psychologically un-retireable:
  every item competes for maintainer attention with the next item,
  indefinitely. With it, every item has a known win condition that
  retires it from rubyrs's maintenance budget.

## ADR 0019 Rule 6 carve-out

ADR 0019 Rule 6 says: "pure-Ruby canon is the reference; native is the
accelerator." For framework batteries (Sinatra, ActiveSupport, future
Rails-component reimpls), Rule 6 as-written is unimplementable — there
is no rubyrs-maintained pure-Ruby form of "Sinatra" to be the canon, and
parity-testing the vendored reimpl against the vendored reimpl is
trivially true.

The carve-out: **for framework batteries the upstream gem IS the canon.**
The rubyrs vendored reimpl is parity-tested against the gem (M27
harness), not maintained as a co-equal pure-Ruby form. The ADR 0019
deviation taxonomy gets a new class `i`:

> **i. Framework-battery reimplementation.** rubyrs's vendored
> implementation of a framework whose upstream gem is the parity
> oracle. Deviation list enumerates exactly where the reimpl
> intentionally diverges (PR #315's `sinatra_lite.rb` doesn't model
> all of Sinatra's `set` semantics, for instance — that's a class-i
> divergence, listed in the item's ADR).

This delta to ADR 0019 lands in the per-menu-item ADR for the first
framework battery (Sinatra) rather than reopening ADR 0019 immediately;
the precedent in that ADR establishes the carve-out for items 4–5
without an extra ADR cycle.

## Consequences

**Positive**

- Sinatra ships as a *real* deliverable (`require "rubyrs/sinatra"` +
  parity CI) rather than an evergreen "soon."
- Each menu item is bounded work with a clear done-state AND a clear
  sunset condition.
- The parity-by-test rule keeps "same code runs on both" from rotting.
- Engine fixes for item N help every later item (compound: PR #315's
  six fixes are pure Tier-1 language wins).
- The public `rubyrs-parity` CLI converts ecosystem-size from a
  weakness into a developer-facing surface that scales without
  rubyrs maintainer effort.

**Negative**

- **Maintainer burden is real and unfunded.** Sinatra-class items
  cost an estimated 0.25–0.5 FTE/item steady-state once upstream-bump
  tracking is included. Six menu items = 1.5–3 FTE forever. v2's
  Open-the-harness move and Exit clause partially absorb this; the
  residual cost needs explicit budgeting per item. **No menu item
  ships without a named owner.**
- **License vetting** is a per-item gate. PR for entry must include
  the upstream gem's license + a confirmation it permits a vendored
  reimpl. MIT/BSD/Apache automatic; LGPL/AGPL-adjacent → ADR cycle
  with maintainers.
- **CI cost grows multiplicatively** in menu items × upstream Ruby
  versions × matrix. Mitigation: tiered parity (smoke matrix runs
  per PR, full matrix runs nightly); aggressive caching of
  `bundle install`; the Open-the-harness move shifts the heaviest
  matrices off the rubyrs CI to gem authors who care.
- **No silent off-menu fallthrough**: refusal at `require` time is
  the default per §"Transitive-deps closure." User experience
  trades immediate `LoadError` for unreachable-code-path
  `NoMethodError`s deep in a request.

**Deferred (explicitly)**

- Rails / ActiveRecord. Tier-4 bet; **conceded**, not just deferred.
- C-extension gems. Out of scope for the menu; the existing `cext`
  feature flag covers the embedding case.
- GAP #13 / `_full_eval` for templating. M27 lands first; templating
  ADR comes after items 1–4 ship and the real demand profile is
  visible.

## References

- PR #315 — Tier-1 language fixes (Sinatra PoC discovery vehicle)
- `poc/sinatra/` — byte-identical app, vendored micro-Sinatra,
  verify harness, full gap log (`GAPS.md`)
- ADR 0015 — concentric architecture; names Tier 2 "Sinatra/Rack capable"
- ADR 0017 — Tier 1 boundary; the surface menu items build atop
- ADR 0019 — implementation-locus axis; native batteries; `require
  "rubyrs/<name>"` Rule 8; the Rule 6 carve-out described above lands
  here in its v-next revision
- ADR 0022 — `_http_server` battery (the Rack mechanism this builds on)
- ADR 0023 — true-async streaming (the Fiber boundary)
- 4-agent review reports (2026-05-31) — architecture / risk /
  competitive / execution-path angles; v2 incorporates their findings
- M27 milestone (see §"Critical path") — the next ship target
