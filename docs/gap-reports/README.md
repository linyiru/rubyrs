# Gap reports

Snapshots of `rubyrs-gapscan` runs against real-world Ruby
codebases. Each report quantifies how far the target sits outside
the rubyrs supported subset and exposes the specific Prism node
classes (and bareword method calls) that are blocking it.

These exist to feed [`ROADMAP.md`](../ROADMAP.md): when a feature
appears at the top of multiple targets' "missing classes" tables,
it's the next thing worth implementing.

## Current reports

| Codebase | Files | % Supported | Top blocker |
|---|---:|---:|---|
| [Jekyll `lib/`](jekyll.md) | 89 | 81.65% | `ModuleNode` (×145) |
| [Liquid `lib/`](liquid.md) | 64 | 81.16% | `ConstantWriteNode` (×141) |

Planned next scans (and the rationale for each) live in
[`TARGETS.md`](TARGETS.md).

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
