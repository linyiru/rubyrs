# 0026: Omakase blessed-gem menu — curated reimplementations, not gem hosting

## Status

**Proposed (2026-05-31).** Grew out of the Sinatra-on-rubyrs PoC in
`poc/sinatra/` and the engine fixes shipped in PR #315 (non-local return
unwind, in-scope rescue, bare `raise`, `Kernel#catch/#throw`, rescue
lexical-scope resolution, String#split limit form, `rack.input` wiring).

Promotes the strategy note that originally lived at
`poc/sinatra/STRATEGY.md` into a first-class architectural decision so
its rules become enforceable across menu items (parity-by-test gate,
honest LoadError on miss, named resolution, published menu).

Sources: this repo's own ADRs (0015, 0019, 0022, 0023), `docs/ROADMAP.md`
gapscan data, and the empirical PoC results — internal research, not
web-cited.

## Context

The recurring question "can rubyrs run gems?" has two pole answers, both
wrong:

- **Pole A: Bundler + every gem (Tier 4).** Resolves arbitrary
  `Gemfile`s, compiles C extensions. Artichoke died trying. ADR 0015 §
  "Why not Artichoke?" cites this. Multi-year, single-maintainer-killing,
  C-ext-ABI bet.
- **Pole B: Pure embedding subset forever (mruby pole).** Permanently
  forecloses the web-framework conversation the embedding brief is asking
  about.

ADR 0015 already named "Tier 2 (`language`) — Sinatra/Rack capable" as
the deliberate position between those poles, and ADR 0019's
implementation-locus axis + native batteries (`_http_server` already
exists and works) is the exact mechanism. This ADR closes the question
of how to *populate* that tier.

The Sinatra PoC (`poc/sinatra/`) is the forcing function: one
byte-identical `app.rb` runs on CRuby + the real `sinatra` gem AND on
rubyrs + a vendored micro-Sinatra, producing byte-identical responses
across 16 routes. The application source never branches on the runtime.

## Decision

Adopt **"omakase = a parity-tested menu of blessed reimplementations"**
as the official answer to "can rubyrs run gems?":

1. **Don't host gems. Bless a menu.** The omakase move is *not* "run any
   gem via Bundler" — that's the Tier 4 bet. It is a small, curated set
   of **rubyrs-native reimplementations** of high-value gems' public
   surface — vendored, integration-tested, namespaced, chosen by the
   maintainers. This PoC's `sinatra_lite.rb` is the prototype of one
   menu item.

2. **The architecture already says yes.** Not proposing new architecture
   — proposing to *populate* the Tier 2/3 surface ADR 0015 + 0019
   already named.

3. **The quality bar is parity, not coverage.** Every blessed library
   must pass a `diff_cruby`-style test against the **real gem** on a
   pinned app. "Same code runs on CRuby and rubyrs" becomes a CI gate,
   not a slogan. PR #315's `poc/sinatra/verify.sh` is the prototype
   harness.

4. **Sinatra now; Rails later (much later).** Sinatra/Rack is reachable
   today — the PoC runs. Rails is a Tier-4 bet behind ActiveRecord, a
   huge metaprogramming surface, and C-exts. Treat them as different
   weight classes, publicly.

5. **Engine fixes from PR #315 unlocked "real" Sinatra apps**: non-local
   `return` across Rust-rooted Rack calls (GAP #1), `rack.input` wiring
   (GAP #2), `Kernel#catch/#throw` (GAP #8), module-nested rescue
   constants (GAP #10), in-scope rescue across Rust-driven blocks
   (GAP #11), bare `raise` re-raise (GAP #14). Everything else the DSL
   needs already works. Templating (ERB/Tilt) needs binding-capturing
   `eval` — Tier 2 `_full_eval` boundary (ADR 0019).

## The omakase menu — opinionated priority order

Ordered by *value ÷ cost*, using the PoC and gapscan as cost evidence.
Mechanism column maps each to ADR 0019's tiers.

| # | Menu item | Why | Mechanism / tier | Readiness |
|---|---|---|---|---|
| 0 | **Rack contract** (`[status,headers,body]`, env) | everything web sits on it | `_http_server` battery (Tier 3) | **shipped** (ADR 0022) |
| 1 | **Sinatra** (modular DSL, routing, params, filters, `halt`, templates-lite) | smallest real framework; the on-ramp | vendored `rubyrs/sinatra` (Tier 2/3) on `_http_server` | **PoC works** post-PR #315; templating GAP #13 still pending |
| 2 | **JSON** (parse/generate) | every API needs it; `as_json` | pure-Ruby canon + `_json_native` accel | ADR 0019 names it; pure form is Tier 3 |
| 3 | **A minimal ActiveSupport core-ext slice** (`blank?`, `present?`, `Hash#deep_*`, `String#camelize` …) | the connective tissue every Ruby web app assumes | pure-Ruby vendored | mostly Tier-1 language; low risk |
| 4 | **A data layer on `_sqlite`** (a Sequel-lite / ROM-lite query DSL, *not* ActiveRecord) | "Sinatra + a DB" is the first real app | `_sqlite` battery (Tier 3, ADR 0019 names it) + pure-Ruby DSL | battery designed, not built |
| 5 | **ERB-lite / Tilt-lite templating** | views | pure-Ruby; gapscan already scans Tilt | Tier 2; `BlockParameterNode` gap |
| — | **Rails / ActiveRecord** | the dream | Tier 4 bet | **explicitly deferred** |

The line between #5 and Rails is the honest edge of the map. Everything
above it is "curate + parity-test"; Rails is "open research."

## The compatibility contract

The brief's real requirement — *"the same code runs on CRuby and
rubyrs"* — becomes the **governing constraint** for every menu item,
enforced mechanically:

1. **Parity-by-test.** For each blessed library, pin a representative
   app (like this PoC's `app.rb`) and a route/behaviour matrix. CI runs
   it on *both* the real gem (CRuby) and the rubyrs reimpl and diffs the
   observable output. Generalises the existing `tests/diff_cruby.rs`
   harness from language fixtures to *framework* fixtures. The PoC's
   `verify.sh` is the seed.

2. **Honest resolution.** `require "sinatra"` resolves to the blessed
   reimpl only when the real gem isn't loadable; `require
   "rubyrs/sinatra"` always pins the built-in (ADR 0019 Rule 8). No
   silent shadowing — when something's off-menu, raise a clear
   `LoadError`, never a degraded fake.

3. **A published menu + parity %.** Ship a table: "Sinatra — on the
   menu, N/M behaviours at parity." gapscan already produces the AST
   view; the parity suite produces the *behaviour* view. Together
   they're an honest marketing surface that doesn't overpromise.

## The opinionated framing: "omakase" means a *menu*, not a *market*

Rails popularised "omakase" (DHH): the maintainers pick a coherent,
integration-tested default stack so you don't assemble one. rubyrs takes
that literally and **inverts the gem model**:

| | CRuby / Bundler | rubyrs omakase |
|---|---|---|
| Sourcing | any of 180k gems | a small blessed menu |
| Trust | per-gem, user's problem | maintainer-curated, one suite |
| Install | resolve + compile C-exts | already in the binary (a feature flag) |
| Compatibility | "should work" | parity-tested vs the real gem in CI |
| Failure mode | dependency hell | "not on the menu yet" (honest LoadError) |

Same posture as Bun/Deno: a curated set of **built-ins** (`bun:sqlite`,
`node:test`) that are *better-integrated than the ecosystem equivalents
because they ship with the runtime* — which ADR 0019 Rule 8 already
adopts (`require "rubyrs/<name>"`). The menu is the product. "We support
Sinatra" should mean "Sinatra is on the menu and parity-tested," not
"paste your Gemfile and pray."

## Consequences

**Positive**

- Sinatra ships as a *real* deliverable (`require "rubyrs/sinatra"` +
  parity CI) rather than an evergreen "soon."
- Each menu item is bounded work with a clear done-state, not an
  open-ended chase.
- The parity-by-test rule keeps "same code runs on both" from rotting
  into "used to work, who knows now."
- Engine fixes for menu item N tend to help every later item — see the
  PR #315 fixes, all pure Tier-1 language wins.

**Negative**

- Maintainer burden per menu item: vendored sources stay in sync with
  upstream gem releases (versioning + a deprecation policy needed).
- "Curated menu" means a finite scope — users on the wrong side of the
  menu get a clean LoadError, not a hack. Has to be documented as a
  feature, not apologised for.
- Pinning real-gem versions in CI means scheduled upstream-bump churn
  (mirroring `bundle update`).

**Deferred (explicitly)**

- Rails / ActiveRecord. Tier-4 bet on the far side of `_sqlite` +
  metaprogramming budget.
- C-extension gems (any gem with native code). Out of scope for the
  menu; the existing `cext` feature flag covers the embedding case.

## Highest-leverage engine work (from the PoC gap log)

If the goal is "a real Sinatra app, not just hello-world," these are the
unlocks, in order. PR #315 closed items 1, 2 + four others; remaining:

1. ~~**GAP #1 — non-local `return` across the Rust-rooted Rack call.**~~
   Fixed in PR #315 (`fix(vm): non-local return + exception unwind
   across Rust-invoked blocks`).
2. ~~**GAP #2 — `rack.input` wiring.**~~ Fixed in PR #315
   (`feat(_http_server): wire env["rack.input"] as a StringIO`).
3. **GAP #4 — an honest runtime discriminator** (`RUBY_ENGINE` or a
   `RUBYRS` constant). Without it, libraries can't choose a rubyrs path
   when they must. Cheap; unblocks the whole "library adapts itself"
   pattern that omakase reimplementations will lean on.
4. The gapscan tail: the **block-parameter family** (`BlockParameterNode`,
   `RestParameterNode`, `SplatNode`) — the #1 remaining AST blocker
   across Sinatra/Tilt/dry-struct.
5. **GAP #13 — `eval` doesn't capture surrounding binding;
   `Kernel#binding` absent.** Blocks templating (ERB/Tilt). Tier 2
   `_full_eval` boundary per ADR 0019.

Items 3-4 are pure Tier-1 language wins that help *every* future menu
item — they compound.

## References

- PR #315 — Tier-1 language fixes (Sinatra PoC discovery vehicle)
- `poc/sinatra/` — the byte-identical app, vendored micro-Sinatra,
  verify harness, full gap log (`GAPS.md`)
- ADR 0015 — concentric architecture; names Tier 2 "Sinatra/Rack capable"
- ADR 0019 — implementation-locus axis; native batteries; the
  `require "rubyrs/<name>"` rule
- ADR 0022 — `_http_server` battery (the Rack mechanism this builds on)
- ADR 0023 — true-async streaming (the Fiber boundary)
