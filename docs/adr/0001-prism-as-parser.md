# 0001: Prism as the parser

## Status

Accepted (2026-05).

## Context

Implementing a Ruby parser is a multi-month project on its own. Ruby's
grammar has notorious ambiguities (`def foo() end`, `foo a b`, heredocs,
`%w[]` etc.) that took decades to settle in `parse.y`. Any new Ruby
implementation either writes its own parser, reuses an existing one, or
shells out to CRuby.

We considered:

1. **Hand-write a Ruby parser in Rust.** Months of work; will diverge
   from CRuby in subtle ways; never finished.
2. **`lib-ruby-parser`** (pure Rust). Mature but **archived** by its
   maintainer in 2024.
3. **Prism** (Ruby's official parser, written in C). Shared upstream by
   CRuby 3.3+, JRuby 9.4+, TruffleRuby. Backed by Shopify. Has a stable
   Rust binding crate `ruby-prism`.

## Decision

Use **Prism** via the `ruby-prism` crate. We add an `Expr` IR layer
immediately after parsing so that nothing downstream depends on Prism's
`'pr` lifetime.

## Consequences

Wins:

- ~5% of the runtime work done for free — and Prism is the hardest 5%.
- Semantics by construction match CRuby's: any edge case in `parse.y` is
  the same edge case in Prism.
- When CRuby's grammar evolves (Ruby 3.5, 4.0), bumping a Cargo dep
  catches us up.
- We become parser-compatible with JRuby, TruffleRuby, Sorbet — anything
  that builds on Prism.

Costs:

- We FFI to C. `ruby-prism-sys` brings `bindgen`, `cc`, and the vendored
  Prism C source. Build is slower than pure Rust.
- WASM compilation needs a `__wasi_init_tp` stub workaround (see
  `build.rs`). Not Prism's fault, but the C dependency surfaces it.
- We don't control the parser; if upstream regresses we wait for a fix.

We accept these because the alternative (hand-writing) is *years* of
work to reach behavioural parity, and `lib-ruby-parser` is unmaintained.
