# Changelog

## 0.1.0 (unreleased)

Initial release.

- Drop-in alternative lexing backend for rouge: `require "blusher"` routes
  `Rouge::RegexLexer#lex` through the Rust `carmine` engine, with byte-identical
  output or transparent fallback to rouge.
- Native engine via an rb-sys/magnus extension (builds the `[Token, value]`
  pair array directly as Ruby objects); a Fiddle-loaded `carmine-ffi` cdylib
  is kept as a dependency-light fallback.
- Callback-free **routability allowlist**: lexers whose tables contain rouge
  `proc` rules carmine can't execute are skipped up front, so they never pay a
  wasted native-lex attempt.
- **Fused lex+format HTML path**: for an unadorned `Rouge::Formatters::HTML`
  pipeline (`Rouge.highlight`/Jekyll/kramdown default), `lex` returns a deferred
  token stream and the patched `HTML#format` lexes AND formats in Rust, returning
  one String — crossing the Ruby boundary once instead of per token. Other
  formatters, the block form, and direct token consumers fall back to rouge.
- Thread-local cache of parsed/compiled tables (the ~0.5 ms build no longer
  repeats per call).
- Correctness: rouge v5.0.0 full lexer spec suite — 757 runs, 5130 assertions,
  0 failures; fused HTML output byte-identical across all 126 routable visual
  samples + rouge's HTML formatter specs.
- Performance: ~1.7× faster highlighting to HTML on a mixed real corpus
  (2.5–2.7× on individual files), parity for callback lexers / non-HTML output.
  (A token stream alone is only ~1.0×: Ruby object allocation at the boundary
  dominates and both engines pay it — fusing the HTML output is what wins.)
