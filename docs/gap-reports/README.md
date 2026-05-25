# Gap reports

Snapshots of `rubyrs-gapscan` runs against real-world Ruby
codebases. Each report quantifies how far the target sits outside
the rubyrs supported subset and exposes the specific Prism node
classes (and bareword method calls) that are blocking it.

These exist to feed [`ROADMAP.md`](../ROADMAP.md): when a feature
appears at the top of multiple targets' "missing classes" tables,
it's the next thing worth implementing.

## Current reports

### Tier 1 (TARGETS.md) — framework / DSL / template

| Codebase | Files | % Supported | #1 missing | Shape |
|---|---:|---:|---|---|
| [Jekyll `lib/`](jekyll.md) | 89 | 84.03% | `ConstantWriteNode` (×70) | static-site framework |
| [Liquid `lib/`](liquid.md) | 64 | 83.07% | `ConstantWriteNode` (×141) | template engine |
| [Sinatra `lib/`](sinatra.md) | 7 | 82.46% | `BlockParameterNode` (×58) | web DSL |
| [dry-struct `lib/`](dry-struct.md) | 15 | 82.38% | `BlockParameterNode` (×11) | modern data DSL |
| [Rake `lib/`](rake.md) | 44 | 83.30% | `RegularExpressionNode` (×42) | task DSL |

### Tier 2 (TARGETS.md) — Bundler / Tilt / stdlib slice

| Codebase | Files | % Supported | #1 missing | Shape |
|---|---:|---:|---|---|
| [Bundler `bundler/lib`](bundler.md) | 225 | **85.25%** | `KeywordHashNode` (×430) | gem dependency DSL |
| [Tilt `lib/`](tilt.md) | 38 | 84.57% | `BlockParameterNode` (×18) | template multiplexer |
| [stdlib `set.rb`](stdlib-set.md) | 1 | 80.70% | `AliasMethodNode` (×16) | stdlib Set |
| [stdlib `optparse.rb`](stdlib-optparse.md) | 1 | 82.73% | `RestParameterNode` (×37) | stdlib OptionParser |
| [stdlib `uri/`](stdlib-uri.md) | 14 | 84.72% | `ConstantWriteNode` (×53) | stdlib URI |

Planned next scans (and the rationale for each) live in
[`TARGETS.md`](TARGETS.md).

## Cross-codebase observations (n=10)

After scanning ten codebases (Tier 1 framework set + Tier 2
Bundler / Tilt / stdlib slice) the picture has converged enough
to draw a few stable conclusions:

- **All ten sit at 80.7–85.3% Supported at AST level.** Different
  shapes (framework / template engine / DSL / dependency manager /
  stdlib) cluster within ~5 pp of each other — the remaining
  ~15–19% is the actual subset gap, not a per-project artifact.
  **Bundler tops the list at 85.25%** despite being by far the
  largest scan target (225 files, 106k AST nodes); size doesn't
  hurt translatability.
- **The #1 missing class is now diverse — no single dominant
  blocker** (was 3/5 ModuleNode at PR #7 baseline, before master
  landed `Module + extend`):
  - `ConstantWriteNode` — 3/10 (Jekyll, Liquid, stdlib URI):
    top-level `FOO = ...`
  - `BlockParameterNode` — 3/10 (Sinatra, dry-struct, Tilt):
    `def foo(&block)` parameter slot
  - `KeywordHashNode` — 1/10 BUT ×430 in Bundler: kwargs-heavy
    modern Ruby
  - `RegularExpressionNode` — 1/10 (Rake), widespread in others
  - `AliasMethodNode` — 1/10 (stdlib set): the `alias new old`
    keyword form (×16 in `set.rb`). The method-call form
    `alias_method :new, :old` parses as `CallNode` and so isn't
    counted here; that's a separate semantic-gap issue.
  - `RestParameterNode` — 1/10 (stdlib optparse): `*args`
- **Block / rest / splat parameter family confirmed broadly** —
  now seen as a major blocker in Sinatra, dry-struct, Tilt, plus
  stdlib optparse and set. Universal DSL-host theme, not just a
  web-framework artifact. `BlockArgumentNode` (the `&block` /
  `&:method` arg site) was Sinatra's #1 at PR #7 baseline; master's
  `&:method_name` (symbol-to-proc) landing reclassified it as
  Supported, which is why Sinatra jumped +2.3 pp.
- **`KeywordHashNode` is the surprise top contender for "next
  highest leverage"** — Bundler alone has ×430. Bundler is also
  the codebase rubund will need to run; this isn't theoretical.
- **`RegularExpressionNode`** is in the Missing tables of 9/10
  codebases (only stdlib `set.rb` lacks it), but a much bigger
  semantic lift than the other top blockers.
- **`AliasMethodNode`** newly visible in stdlib set (×16). Pure
  static-method-aliasing form (no metaprogramming required to
  implement) — could be a quick win if any other future target
  surfaces it heavily.

### Gap-closing feedback loop

These reports are snapshots, not commit-by-commit history. After a
batch of features lands, regenerate the reports and update the
table above. The previous snapshot (PR #7, n=5) showed 79–82%; a
broad feature batch on master moved every codebase up by 1–2 pp:

| Codebase | PR #7 baseline | Current | Δ |
|---|---:|---:|---:|
| Jekyll | 81.65% | 84.03% | +2.38 |
| Liquid | 81.16% | 83.07% | +1.91 |
| Sinatra | 79.85% | 82.46% | +2.61 |
| dry-struct | 80.13% | 82.38% | +2.25 |
| Rake | 80.87% | 83.30% | +2.43 |

For per-feature impact use `gapscan diff before.json after.json` —
PR #9 fixed a bug where `diff` re-classified both sides with the
current classifier, masking cross-version movement. Reports
generated from PR #9 onwards carry per-class scan-time
classification and `diff` honours it.

## Why these numbers under-state the gap

The headline "% Supported" is **AST-level only** and is an
*upper bound* on translatability. Many features that rubyrs does
NOT implement parse as `CallNode` (which counts as Supported):

- `require`, `autoload` — semantically forbidden in rubyrs
- `attr_reader / writer / accessor` — Near term #6 in [ROADMAP](../ROADMAP.md), not yet implemented
- `include`, `extend` — coupled to `Module` support
- `private`, `public`, `protected`

The "Top bareword calls" section of each report exposes these
hidden gaps. Use both views together when sizing work.

## Regenerating

```bash
# 1. Clone the target codebase somewhere outside the workspace
git clone --depth=1 https://github.com/jekyll/jekyll.git /tmp/jekyll
git clone --depth=1 https://github.com/Shopify/liquid.git  /tmp/liquid

# 2. Build gapscan in release mode (counts honest perf)
cargo build --release -p rubyrs-gapscan

# 3. Scan into Markdown
./target/release/rubyrs-gapscan scan /tmp/jekyll/lib --format md --top 50 > /tmp/jekyll-body.md
./target/release/rubyrs-gapscan scan /tmp/liquid/lib  --format md --top 50 > /tmp/liquid-body.md

# 4. Manually prepend the reproducibility header (rubyrs SHA +
#    target SHA + scan date) so the report stays interpretable
#    after either side moves. See jekyll.md for the template.
```

## Diffing successive runs

JSON scans diff cleanly across rubyrs versions — handy after each
gap-closing PR:

```bash
./target/release/rubyrs-gapscan scan /tmp/jekyll/lib --format json -o before.json
# ... land a feature ...
./target/release/rubyrs-gapscan scan /tmp/jekyll/lib --format json -o after.json
./target/release/rubyrs-gapscan diff before.json after.json --format md
```

The diff report surfaces closed-gap classes and per-method
bareword deltas — quantifies the impact of each feature PR.
