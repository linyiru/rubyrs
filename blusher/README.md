# blusher

A **faster, drop-in, byte-for-byte-compatible** alternative backend for Ruby's
[rouge](https://github.com/rouge-ruby/rouge), powered by the Rust
[`carmine`](https://crates.io/crates/carmine) engine.

`require "blusher"` routes rouge's lexing — and, for the common case, its HTML
formatting — through carmine, which executes rule tables extracted from rouge's
own lexers. carmine either produces **byte-identical** output or **declines**,
in which case blusher falls back to rouge unchanged. Zero code change, zero
divergence; **~1.7× faster** highlighting to HTML on a mixed corpus (more on
large files), parity elsewhere.

```ruby
require "rouge"
require "blusher"   # ← that's it

# The hot path: lexing + HTML formatting fused in Rust, one String returned.
html = Rouge.highlight(File.read("data.json"), "json", "html")
```

## Performance

**For the HTML-highlighting path — what rouge is overwhelmingly used for —
blusher is ~1.7× faster on a mixed real corpus and 2.5–2.7× on individual
files**, with byte-identical output. Measured over rouge's own 126 routable
visual sample files (690 KiB) rendered to HTML with `Rouge::Formatters::HTML`:

```
              ms/pass    MB/s
  rouge        187.1      3.8
  blusher      110.2      6.4     → 1.70×
```

The trick is **not** faster lexing in isolation. A Ruby lexer's cost is
dominated by allocating Ruby objects at the boundary — one String + one Array
per token — which both engines pay identically, so `lex` alone is only ~1.0×
even though carmine's core is ~4.6× faster Rust-to-Rust. But a token stream is
just an *intermediate*: the real output is an HTML string. So blusher **fuses
lexing and HTML formatting in Rust and returns one String**, crossing the Ruby
boundary once (O(1)) instead of once per token (O(n)). That is where carmine's
speed finally shows up end-to-end. (Two supporting wins: a thread-local cache of
the parsed/compiled table so the ~0.5 ms build doesn't repeat per call, and
resolving CSS class names in Rust.)

Scope and honesty:

- The fast path applies when the whole pipeline is `format(lex(src))` with an
  **unadorned `Rouge::Formatters::HTML`** (the `Rouge.highlight` / Jekyll /
  kramdown default). Subclasses (HTMLInline, HTMLTable, Pygments, …), the
  token-streaming block form, and direct token consumers transparently fall
  back to rouge — same output, no speedup.
- It applies to the **126 of 227 callback-free lexers** (JSON, SQL, YAML, CSS,
  many config/markup formats). The other 101 use rouge `proc` rules carmine
  can't execute; blusher detects this up front and runs rouge for them (parity,
  no wasted work). So a JSON/SQL/config-heavy workload wins big; a
  Ruby/Python-heavy one trends toward parity.
- carmine's raw 4.6× is fully realized only Rust-to-Rust (e.g. embedded in
  [rubyrs](https://github.com/linyiru/rubyrs), no Ruby boundary at all).

## How it works

- `require "blusher"` aliases the original `Rouge::RegexLexer#lex` to
  `__blusher_rouge_lex` and replaces it. For a routable lexer (table exists and
  is callback-free), `lex` without a block returns a deferred
  `Blusher::Shim::TokenStream` holding `(lexer, source)` — nothing is lexed yet.
- The patched `Rouge::Formatters::HTML#format` recognises that stream and calls
  the **fused** `Blusher::Engine.format_html`, which lexes *and* formats in Rust
  and returns one HTML String. Any other consumer (a different formatter, the
  block form, `.to_a`) just iterates the stream, which lexes via carmine on
  demand and yields the same `[Token, value]` pairs rouge would.
- carmine **declines** anything it can't reproduce identically (callback rules,
  recursion, …) and blusher falls back to rouge, so the output is always exactly
  rouge's — verified against the full lexer spec suite.
- The native backend is the **rb-sys/magnus extension** (`blusher.{bundle,so}`).
  A `carmine-ffi` + Fiddle path is kept as a dependency-light fallback (it
  marshals tokens through JSON, can't fuse, and is for correctness only).

## Correctness

Verified against rouge v5.0.0's **full lexer spec suite: 757 runs, 5130
assertions, 0 failures** (`rake spec`). The spec suite is the correctness gate —
any new divergence must be fixed in carmine or the rule forced to decline.

## Build (dev, in the rubyrs monorepo)

```sh
rake compile      # build the magnus ext → lib/blusher.<dlext>
rake compile_ffi  # (optional) build the carmine-ffi cdylib fallback → ext/
rake tables       # regenerate lib/blusher/tables/<tag>.json from installed rouge
ROUGE_SRC=/path/to/rouge rake spec
```

## Status

Part of [momiji-rs](https://github.com/momiji-rs) — Rust-backed engines for the
Ruby ecosystem. Tables are derived from rouge (MIT, © Jeanine Adkisson and
contributors).
