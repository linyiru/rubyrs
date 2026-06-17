# blusher

A fast, **rouge-compatible** syntax highlighter, backed by the Rust
[`carmine`](https://crates.io/crates/carmine) engine.

`blusher` routes Ruby's [rouge](https://github.com/rouge-ruby/rouge) lexers
through carmine, which executes rule tables extracted from rouge's own lexers.
Where carmine produces a **byte-identical** token stream it accelerates lexing
(~4.6× median faster than rouge); everywhere else it transparently **falls back
to rouge** — zero code change, zero divergence.

```ruby
require "rouge"
require "blusher"   # ← that's it; rouge lexing now goes through carmine where it can

html = Rouge::Formatters::HTML.new.format(
  Rouge::Lexers::Ruby.new.lex("def hi = puts('hello')")
)
```

## How it works

- `require "blusher"` monkeypatches `Rouge::RegexLexer#lex`. For each lex it
  looks up the lexer's carmine table; if carmine returns `ok`, those tokens are
  used; on `decline`/`error` it calls the original rouge `lex`.
- carmine **declines** any input it can't lex identically to rouge (callback
  rules, recursive regexes, …), so the output is always exactly rouge's.

## Correctness

Verified against rouge v5.0.0's **full lexer spec suite: 757 runs, 5130
assertions, 0 failures** (run via `rake spec`). The spec suite is the
correctness gate — any new divergence must be fixed in carmine or the rule
forced to decline.

## Build (dev, in the rubyrs monorepo)

```sh
rake compile   # build the carmine-ffi cdylib → ext/
rake tables    # generate lib/blusher/tables/<tag>.json from the installed rouge
ROUGE_SRC=/path/to/rouge rake spec
```

## Status / roadmap

This is the FFI/Fiddle bootstrap. The release path:

1. Swap Fiddle → an **rb-sys/magnus** native extension (precompiled
   cross-platform binaries, ergonomic token marshaling).
2. Bundle `lib/blusher/tables/*.json` in the gem.
3. Wire rouge's spec suite as the CI correctness gate.

Part of [momiji-rs](https://github.com/momiji-rs) — Rust-backed engines for the
Ruby ecosystem. Tables are derived from rouge (MIT, © Jeanine Adkisson and
contributors).
