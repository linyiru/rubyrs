# Gap reports

Snapshots of `rubyrs-gapscan` runs against real-world Ruby
codebases. Each report quantifies how far the target sits outside
the rubyrs supported subset and exposes the specific Prism node
classes (and bareword method calls) that are blocking it.

These exist to feed [`ROADMAP.md`](../ROADMAP.md): when a feature
appears at the top of multiple targets' "missing classes" tables,
it's the next thing worth implementing.

## Current reports

| Codebase | Files | % Supported | #1 missing | Shape |
|---|---:|---:|---|---|
| [Jekyll `lib/`](jekyll.md) | 89 | 83.21% | `ModuleNode` (×145) | static-site framework |
| [Liquid `lib/`](liquid.md) | 64 | 82.12% | `ConstantWriteNode` (×141) | template engine |
| [Sinatra `lib/`](sinatra.md) | 7 | 82.19% | `BlockParameterNode` (×58) | web DSL |
| [dry-struct `lib/`](dry-struct.md) | 15 | 81.54% | `ModuleNode` (×16) | modern data DSL |
| [Rake `lib/`](rake.md) | 44 | 82.39% | `ModuleNode` (×52) | task DSL |

Planned next scans (and the rationale for each) live in
[`TARGETS.md`](TARGETS.md).

## Cross-codebase observations (n=5)

After scanning five codebases the picture has converged enough to
draw a few stable conclusions:

- **All five hover at 81–83% Supported at AST level.** Different
  shapes (framework / template engine / DSL), same headline. The
  remaining ~17–19% is the actual subset gap, not a Jekyll-specific
  quirk. (Initial snapshot was 79–82%; a batch of master features
  — `unless`/`until`, `**`, `Hash#sort_by`, `String#[]`/`String#[]=`,
  more `Float`/`Kernel#p`/visibility/`op_assign`/`Range`/
  `Comparable`/`String#match`, inline `rescue`, `__method__`,
  Inspect round-out, `&:method_name` — moved the band up 1.0–2.3 pp
  across the board.)
- **`ModuleNode` is the #1 missing class in 3/5 scans** (Jekyll,
  dry-struct, Rake) and #2 in Liquid. Sinatra is the outlier
  (single `module Sinatra` so it ranks low there). This confirms
  ROADMAP Near term #4 as the highest-leverage feature.
- **Block / rest / splat parameter family** (`BlockParameterNode`,
  `RestParameterNode`, `SplatNode`) dominates Sinatra and shows up
  heavily in dry-struct and Rake. These are the *DSL framework*
  blockers — Jekyll/Liquid don't surface them because they're more
  "library" than "DSL host". `BlockArgumentNode` (the `&block` /
  `&:method` arg site) was the heaviest of these on Sinatra at PR #7
  baseline; master's `&:method_name` (symbol-to-proc) landing
  reclassified it as Supported, which is why Sinatra jumped +2.3 pp.
- **`RegularExpressionNode`** is widespread (4/5) but a much
  bigger semantic lift than the others.

### Gap-closing feedback loop

These reports are snapshots, not commit-by-commit history. After a
batch of features lands, regenerate the reports and update the
table above. The previous snapshot (PR #7, n=5) showed 79–82%; a
broad feature batch on master moved every codebase up by 1–2 pp:

| Codebase | PR #7 baseline | Current | Δ |
|---|---:|---:|---:|
| Jekyll | 81.65% | 83.21% | +1.56 |
| Liquid | 81.16% | 82.12% | +0.96 |
| Sinatra | 79.85% | 82.19% | +2.34 |
| dry-struct | 80.13% | 81.54% | +1.41 |
| Rake | 80.87% | 82.39% | +1.52 |

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
