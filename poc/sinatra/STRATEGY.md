# Opinionated & omakase: how rubyrs could support gems like Sinatra/Rails

*A codebase-grounded strategy note. Sources are this repo's own ADRs
(0015, 0019, 0022, 0023), `docs/ROADMAP.md` gapscan data, and the
empirical results of the Sinatra PoC in this directory. This is internal
research, not web-cited.*

---

## TL;DR

1. **Don't host gems. Bless a menu.** The omakase move is *not* "run any
   gem via Bundler" (that's Tier 4 — a multi-year, C-ext-ABI bet). It's a
   small, curated set of **rubyrs-native reimplementations** of
   high-value gems' public surface — vendored, integration-tested, and
   namespaced — chosen by the maintainers. This PoC's `sinatra_lite.rb`
   is the prototype of one menu item.

2. **The architecture already says yes.** ADR 0015 names "Tier 2
   (`language`) — Sinatra/Rack capable" explicitly. ADR 0019's
   implementation-locus axis + native batteries (`_http_server` already
   exists and works) is the exact mechanism. We are not proposing new
   architecture — we're proposing to *populate* it.

3. **The quality bar is parity, not coverage.** Every blessed library
   must pass a `diff_cruby`-style test against the **real gem** on a
   pinned app. "Same code runs on CRuby and rubyrs" becomes a CI gate,
   not a slogan. This PoC's `verify.sh` is the prototype harness.

4. **Sinatra now; Rails later (much later).** Sinatra/Rack is reachable
   today — the PoC runs. Rails is a Tier-4 bet behind ActiveRecord, a
   huge metaprogramming surface, and C-exts. Treat them as different
   weight classes, publicly.

5. **Two engine fixes unlock "real" Sinatra apps:** non-local `return`
   across the Rust-rooted Rack call (GAP #1) and `rack.input` wiring
   (GAP #2). Everything else the DSL needs already works.

---

## What the PoC established

The same `app.rb` — Sinatra modular style, `class App < Sinatra::Base`
with `get`/`post`, path params, instance-context route blocks, a helper
method — produced **byte-identical HTTP responses** on:

- CRuby 3.4.1 + the real **sinatra 4.2.1** gem (puma server), and
- rubyrs + a 120-line vendored micro-Sinatra on the **`_http_server`**
  battery.

The application source never branches on the runtime. The only
engine-aware file is a 25-line `sinatra_compat.rb` that chooses who
provides `Sinatra::Base`.

That is the whole thesis in miniature: **the language surface is already
close enough that a curated reimplementation is indistinguishable from
the real gem for the common path** — and the gaps that remain are
specific and listed (`GAPS.md`), not diffuse.

Corroborating signal from the repo's own tooling: `rubyrs-gapscan`
reports **Sinatra `lib/` is 82.5% AST-supported**, with the top blocker
being the block-parameter family — i.e. the remaining 17% is a *named,
finite* set, not an open-ended chase.

---

## The opinionated framing: "omakase" means a *menu*, not a *market*

Rails popularised "omakase" (DHH): the maintainers pick a coherent,
integration-tested default stack so you don't assemble one. rubyrs should
take that literally and **invert the gem model**:

| | CRuby / Bundler | rubyrs omakase |
|---|---|---|
| Sourcing | any of 180k gems | a small blessed menu |
| Trust | per-gem, user's problem | maintainer-curated, one suite |
| Install | resolve + compile C-exts | already in the binary (a feature flag) |
| Compatibility | "should work" | parity-tested vs the real gem in CI |
| Failure mode | dependency hell | "not on the menu yet" (honest LoadError) |

This is the same posture as Bun/Deno: a curated set of **built-ins**
(`bun:sqlite`, `node:test`) that are *better-integrated than the
ecosystem equivalents because they ship with the runtime* — which ADR
0019 Rule 8 already adopts (`require "rubyrs/<name>"`). The menu is the
product. "We support Sinatra" should mean "Sinatra is on the menu and
parity-tested," not "paste your Gemfile and pray."

### Why this beats both alternatives

- **Beats "host real gems" (Tier 4 now):** Artichoke died trying to
  deliver all of Ruby + C-ext ABI with one maintainer (ADR 0015 cites
  this). A blessed menu is bounded work with a clear done-state per item.
- **Beats "stay a pure embedding subset forever" (mruby pole):** that
  permanently forecloses the web-framework conversation the brief is
  asking about. The concentric architecture exists precisely so we don't
  have to choose the pole.

---

## The omakase menu — opinionated priority order

Ordered by *value ÷ cost*, using the PoC and gapscan as cost evidence.
Mechanism column maps each to ADR 0019's tiers.

| # | Menu item | Why | Mechanism / tier | Readiness |
|---|---|---|---|---|
| 0 | **Rack contract** (`[status,headers,body]`, env) | everything web sits on it | `_http_server` battery (Tier 3) | **shipped** (ADR 0022) |
| 1 | **Sinatra** (modular DSL, routing, params, filters, `halt`, templates-lite) | smallest real framework; the on-ramp | vendored `rubyrs/sinatra` (Tier 2/3) on `_http_server` | **PoC works**; needs GAP #1, #2 |
| 2 | **JSON** (parse/generate) | every API needs it; `as_json` | pure-Ruby canon + `_json_native` accel | ADR 0019 names it; pure form is Tier 3 |
| 3 | **A minimal ActiveSupport core-ext slice** (`blank?`, `present?`, `Hash#deep_*`, `String#camelize` …) | the connective tissue every Ruby web app assumes | pure-Ruby vendored | mostly Tier-1 language; low risk |
| 4 | **A data layer on `_sqlite`** (a Sequel-lite / ROM-lite query DSL, *not* ActiveRecord) | "Sinatra + a DB" is the first real app | `_sqlite` battery (Tier 3, ADR 0019 names it) + pure-Ruby DSL | battery designed, not built |
| 5 | **ERB-lite / Tilt-lite templating** | views | pure-Ruby; gapscan already scans Tilt | Tier 2; `BlockParameterNode` gap |
| — | **Rails / ActiveRecord** | the dream | Tier 4 bet | **explicitly deferred** |

The line between #5 and Rails is the honest edge of the map. Everything
above it is "curate + parity-test"; Rails is "open research."

---

## The compatibility contract (the part that makes it credible)

The brief's real requirement — *"the same code runs on CRuby and
rubyrs"* — should become the **governing constraint** for every menu
item, enforced mechanically:

1. **Parity-by-test.** For each blessed library, pin a representative app
   (like this PoC's `app.rb`) and a route/behaviour matrix. CI runs it on
   *both* the real gem (CRuby) and the rubyrs reimpl and diffs the
   observable output. This generalises the existing `tests/diff_cruby.rs`
   harness from language fixtures to *framework* fixtures. The PoC's
   `verify.sh` is the seed.

2. **Honest resolution.** `require "sinatra"` resolves to the blessed
   reimpl only when the real gem isn't loadable; `require
   "rubyrs/sinatra"` always pins the built-in (ADR 0019 Rule 8). No
   silent shadowing — when something's off-menu, raise a clear
   `LoadError`, never a degraded fake.

3. **A published menu + parity %.** Ship a table: "Sinatra — on the menu,
   N/M behaviours at parity." gapscan already produces the AST view; the
   parity suite produces the *behaviour* view. Together they're an honest
   marketing surface that doesn't overpromise.

---

## The highest-leverage engine work (from the PoC gap log)

If the goal is "a real Sinatra app, not just hello-world," these are the
unlocks, in order:

1. **GAP #1 — non-local `return` across the Rust-rooted Rack call.**
   `collection.each { … return … }` is everywhere in web code; today it
   traps when the method was entered via the Rack lambda. Highest-impact
   correctness fix. (Touches the same machinery as ADR 0024 / 0005.)

2. **GAP #2 — `rack.input` wiring.** No request body = no write-side web
   app. ADR 0022 already specifies the StringIO approach (stage 4c.3);
   ~30-45 LOC + GC-rooting care.

3. **GAP #4 — an honest runtime discriminator** (`RUBY_ENGINE` or a
   `RUBYRS` constant). Without it, libraries can't choose a rubyrs path
   when they must. Cheap; unblocks the whole "library adapts itself"
   pattern that omakase reimplementations will lean on.

4. Then the gapscan tail: the **block-parameter family**
   (`BlockParameterNode`, `RestParameterNode`, `SplatNode`) — the #1
   remaining AST blocker across Sinatra/Tilt/dry-struct.

Items 3-4 are also pure Tier-1 language wins that help *every* future
menu item, not just Sinatra — so they compound.

---

## Recommendation

Adopt "omakase = a parity-tested menu of blessed reimplementations" as
the official answer to "can rubyrs run gems?", and make **Sinatra the
first menu item to ship end-to-end**: fix GAP #1 and #2, promote this
PoC's `sinatra_lite.rb` into a real `rubyrs/sinatra` with its own ADR
(per ADR 0019 Rule 7), and wire the parity harness into CI. That delivers
a genuine, defensible "rubyrs runs Sinatra" claim — same code, both
runtimes — without touching the Tier-4 gem-host black hole. Rails stays a
clearly-labelled bet on the far side of `_sqlite` + ActiveRecord.
