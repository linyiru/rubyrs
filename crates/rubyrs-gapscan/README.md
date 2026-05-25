# rubyrs-gapscan

> **Scans Ruby codebases for AST features
> [`rubyrs`](https://crates.io/crates/rubyrs) cannot translate,
> to quantify and prioritise the subset gap.**

Walks one or many `.rb` files via the
[Prism](https://github.com/ruby/prism) parser, classifies every AST
node as **Supported**, **RidesAlong** (the node only appears inside
something we already support), or **Missing**, and prints — or
emits as JSON — a coverage report.

The output is the canonical answer to the question
*"is the niche we claim to serve actually served?"* for any given
real-world Ruby corpus.

## Install

```bash
cargo install rubyrs-gapscan
```

## Use

```
$ rubyrs-gapscan scan crates/rubyrs/examples/brewfile
Files scanned: 2
Total AST nodes: 277
  Supported:        195 (70.40%)
  RidesAlong:        68 (24.55%)
  Missing:           14 (5.05%)

Missing node classes:
  GlobalVariableReadNode    10  ($taps)
  GlobalVariableWriteNode    4  ($taps = [])
```

For machine-readable output:

```
$ rubyrs-gapscan scan --format json crates/rubyrs/examples/gemfile > report.json
```

## Why it matters

`rubyrs` deliberately implements a Ruby *subset*. `gapscan` turns
that vague subset boundary into a concrete percentage against any
target corpus, so the project can ratchet coverage upward
PR-by-PR. The repo's CI runs `gapscan` against representative
corpora on every PR and posts a diff comment when the supported
percentage moves.

## License

Dual-licensed under [MIT](https://github.com/linyiru/rubyrs/blob/master/LICENSE-MIT)
or [Apache-2.0](https://github.com/linyiru/rubyrs/blob/master/LICENSE-APACHE)
at your option.
