# Changelog

`carmine` is a rouge-compatible syntax-highlighting engine: it executes rule
tables extracted from Ruby's rouge lexers (via `tools/extract.rb`) with
RegexLexer semantics, used as a fast accelerator that DECLINES unsupported
cases to pure rouge. The format follows [Keep a Changelog]; versions are
[semver]. (The `blusher` Ruby gem wraps this crate via `carmine-ffi`.)

[Keep a Changelog]: https://keepachangelog.com/en/1.1.0/
[semver]: https://semver.org/

## [0.3.0] — unreleased

Verified **zero-divergence** against rouge v5.0.0's full lexer spec suite
(757 runs / 5130 assertions / 0 failures) through the lex-or-decline
contract, and ~4.6× median faster than rouge over its real visual-sample
corpus. All changes are backward-compatible additions to the IR vocabulary
plus regex-fidelity fixes (no API breaks).

### Added — Conditional-Action IR vocabulary

- **`recurse`** op — re-lex a substring with the SAME table from a fresh
  `:root` (rouge `recurse` / `delegate(self.class)`), splicing the tokens;
  a sub-lex that hits a callback propagates the decline.
- **Case-fold conditions** `ginf` / `geqf` — `SET.include?(m[i].downcase)` and
  `m[i].downcase == "lit"` (case-insensitive SQL/COBOL-family classifiers).
- **`gmatch`** condition — `m[i] =~ /re/` (unanchored regex match on a group;
  uses a dedicated non-pos-anchored compile).
- **`gpresent`** condition — `if m[i]` (group i participated, even if empty).
- **`sdepth`** condition — `stack.size <cmp> n` (state-stack depth; `cmp` ∈
  eq/ne/lt/le/gt/ge).
- **`and` / `or`** conditions — `c1 && c2`, `c1 || c2`.

### Fixed — regex fidelity to rouge / Onigmo semantics

- **Inline flag translation:** Ruby `(?m:…)` is DOTALL (`.` matches newline);
  mapped to Rust `(?s:…)` so multi-line block comments (`(?m-ix:/\*…\*/)`)
  match.
- **`/x` mode keeps char-class whitespace:** Rust's verbose mode would strip
  the space in `[ \t]` (→ tab-only); now escaped so it survives.
- **`^` is line-anchored:** rouge scans with `StringScanner fixed_anchor:
  true`, so `^` anchors at real line boundaries — matching carmine's
  anchored-at-pos search. Removed the unsound `strip_leading_carets`
  heuristic (it corrupted `(?:^|…)` alternations into always-true).
- **Recursive-subroutine regexes decline:** `\g<…>` (e.g. balanced-brace
  preproc blocks) can't be matched by either engine; such rules now become
  callbacks so carmine declines to rouge rather than mis-match.
- **Embedded NUL in input** (`carmine-ffi`): input is length-delimited, not a
  NUL-terminated C string, so blobs with `\0` are no longer truncated.

## [0.2.0]

Initial crates.io release: the Conditional-Action IR engine, the
linear-first dual regex engine (regex-automata meta + fancy-regex
backtracker with `\G` anchoring), Onigmo char-class rewrites (Unicode POSIX
+ ASCII shorthands, octal-in-class), and DIVERGE→0 over the rouge demo
corpus.
