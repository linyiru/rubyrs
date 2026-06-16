# kramdown-rostdown (PoC)

A **zero-code-change accelerator** for the [kramdown] gem on stock
CRuby/MRI, backed by the Rust [`rostdown`](../../crates/rostdown) renderer.

```ruby
require "kramdown-rostdown"   # the only change

# Everything below is untouched and now runs through rostdown when it can:
Kramdown::Document.new(markdown, input: "GFM").to_html
```

It is the [rubyrs](../../README.md) in-VM `_kramdown_native` accelerator
(`crates/rubyrs/src/kramdown_native_shim.rb`) lifted out of the
interpreter and shipped as an ordinary gem, so the broader Ruby
community — Jekyll, Bridgetown, Middleman, plain scripts — can use it on
the official Ruby.

## The contract: byte-identical or fall back

rostdown reproduces kramdown's HTML **byte-for-byte** for an
explicitly-bounded subset (GFM / kramdown-core flavor, smart typography,
auto-ids, rouge code highlighting). For *anything* outside that subset —
footnotes, math, tables, definition lists, exotic options — it
**declines**, and the gem transparently runs pure-Ruby kramdown for that
document. So the accelerator is **never silently wrong**: output is
either identical or produced by kramdown itself.

Two gates enforce this:

1. **Per-options** (`profile_for`): the merged options hash must match a
   render profile rostdown can reproduce (e.g. `entity_output:
   :as_char`, default `smart_quotes`, GFM ⇒ `hard_wrap: false`, no
   `remove_line_breaks_for_cjk`, …). Anything else → pure Ruby.
2. **Per-document**: rostdown returns `Declined` for any construct
   outside its subset → pure Ruby.

Code highlighting stays identical *by construction*: the Rust side
records each fenced block, Ruby's Rouge produces the inner HTML (the
exact path kramdown's rouge plugin uses), and Rust splices it back into
kramdown's wrapper markup — a two-pass `scan`/`supply`/`render` protocol.

## Results on this machine

`rake spec` — **differential conformance** over kramdown's *own* test
corpora (the kramdown 2.5.2 + kramdown-parser-gfm suites), rendering
every case twice and asserting the accelerated HTML equals pristine
kramdown's:

```
  kramdown-core   198 cases
  gfm              13 cases
  TOTAL           211 cases
  rostdown native hits : 24      (11.4% — the corpus is an adversarial
  rostdown declined    : 149      pile of edge cases, not real prose)
  options ineligible   : 38
  RESULT: PASS — byte-identical to pure kramdown on all 211 cases.
```

> The low native coverage is the corpus doing its job: it is a stress
> test of unusual constructs. Real-world prose (blog posts, docs, READMEs)
> sits almost entirely inside the subset — see the benchmark.

`rake bench` — same `Kramdown::Document#to_html` call, accelerator on vs
off (Ruby 3.4.1, kramdown 2.5.2, rouge 5.0.0, arm64):

| workload | pure kramdown | rostdown accel | speedup |
|---|---:|---:|---:|
| prose post (GFM, no code) | 1,921 i/s | 40,989 i/s | **21.3×** |
| post with code (Jekyll + rouge) | 285 i/s | 490 i/s | **1.7×** |

Prose is a pure win (the entire Ruby parse is skipped). Code-heavy docs
are bottlenecked by **Ruby Rouge**, which both paths share — exactly the
cost rubyrs eliminates with the `carmine` Rust highlighter + static
lexer tables. Bundling that into the gem is the obvious next lever.

## Layout

```
ext/                 Rust cdylib: C ABI over rostdown (rd_scan/supply/render…)
  src/lib.rs         1:1 port of rubyrs' __rubyrs_kd_* host fns
lib/kramdown/rostdown.rb   FFI binding + profile gate + Document patch + Rouge
bin/spec_diff.rb     differential conformance harness
bin/bench.rb         benchmark-ips comparison
```

## Run it

```sh
rake compile          # cargo build --release → ext/target/release/*.dylib
rake spec             # differential conformance (must PASS)
rake bench            # speedups
```

Set `KRAMDOWN_ROSTDOWN_LIB=/path/to/lib.dylib` to override the cdylib
location, and `KRAMDOWN_ROSTDOWN_STATS=1` to print native/decline counts
at exit.

## From PoC to a shippable gem

This PoC uses the `ffi` gem + a prebuilt cdylib because it is the fastest
path to a working demo and keeps `rostdown` dependency-free. To actually
ship to RubyGems the way the community expects (no Rust toolchain on the
user's machine):

- **Bind with [rb-sys] + [magnus]** and build with `rake-compiler`, the
  same stack [commonmarker] (Rust/comrak) uses. The `scan`/`supply`/
  `render` C ABI here maps directly onto magnus method definitions.
- **Cross-compile precompiled gems** (`rake-compiler-dock` /
  `oxidized`) for darwin/linux × arm64/x86_64, with a pure-fallback
  install path.
- **Embed a Rust highlighter** (`carmine`) so code-heavy docs win too.
- **Widen the subset**: GFM `hard_wrap: true`, tables, footnotes — each
  is a tracked, byte-identity-gated increment in rostdown, never a
  correctness gamble.

[kramdown]: https://kramdown.gettalong.org/
[`rostdown`]: ../../crates/rostdown
[rb-sys]: https://github.com/oxidize-rb/rb-sys
[magnus]: https://github.com/matsadler/magnus
[commonmarker]: https://github.com/gjtorikian/commonmarker
