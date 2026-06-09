# carmine

A [rouge](https://github.com/rouge-ruby/rouge)-compatible syntax-highlighting
engine in Rust. *pygments → rouge → carmine.*

carmine does not define lexers of its own. Instead it executes **rule tables
extracted from rouge's lexers** (240+ languages, battle-tested) with the same
state-machine semantics as rouge's `RegexLexer`, and formats tokens with the
same HTML span/escape rules — producing **byte-identical output** to rouge for
the supported rule kinds, at native speed.

- `table`: the serde-free JSON rule-table model (states → ordered rules:
  `tok` / `actions` / `wordlist` / `callback` / `mixin`) plus the token
  shortname registry.
- `engine`: the lexer. Declarative rules run fully natively. Rules that rouge
  defines with match-dependent Ruby blocks are marked `callback`; embedders
  supply a [`Callback`] implementation (e.g. a Ruby VM bridging back to the
  original block) or use [`NoCallbacks`], which makes `lex` return an error so
  the caller can fall back to running rouge itself.
- `html`: the rouge-compatible HTML formatter (escape `&<>`, bare `Text`,
  `<span class="SHORTNAME">` otherwise).

Rule tables are produced by `tools/extract.rb`, which loads rouge and records
each lexer's state definitions through a tracing DSL. Tables derived from
rouge are subject to rouge's MIT license (© Jeanine Adkisson and
contributors); see the fixture headers.
