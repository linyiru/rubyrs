# `rubyrs-spec-extract`

Mechanically rewrites upstream [ruby/spec](https://github.com/ruby/spec)
files into the `assert_eq` / `assert_raises` shape that rubyrs's
micro-runner consumes (`crates/rubyrs/spec/`).

This is the v0.1 implementation of Layer 4 of the testing
strategy
([`docs/TESTING.md`](https://github.com/linyiru/rubyrs/blob/master/docs/TESTING.md)).
v0.1 recognises a single pattern — `expr.should == val` — which is
the bulk of the equality-style `it` blocks in upstream
`core/string`, `core/method`, and similar simple files. The
hand-translated specs in
[`crates/rubyrs/spec/ruby/`](https://github.com/linyiru/rubyrs/tree/master/crates/rubyrs/spec/ruby)
(from PRs #48 / #52 / #55) are the reference shape — running the
extractor on the same upstream sources should produce
similar output, with the leftover patterns (negation,
predicate matchers, `raise` matchers) showing up unchanged
for a human to translate.

## Usage

```bash
cargo run --release -p rubyrs-spec-extract -- path/to/upstream_spec.rb
```

Output goes to stdout. Redirect into `crates/rubyrs/spec/ruby/`
to land it as a new spec file:

```bash
cargo run --release -p rubyrs-spec-extract \
  -- /path/to/ruby-spec/core/string/length_spec.rb \
  > crates/rubyrs/spec/ruby/string_length_spec.rb

# Then sanity-check it runs in the micro-runner:
cargo test -p rubyrs --test ruby_spec
```

## What the extractor recognises (current: v0.2)

The recogniser shipped incrementally. Patterns in italics
are passthrough — extractor leaves them verbatim for a
human polish step.

| Upstream pattern | Rewrites to | Since |
|---|---|---|
| `expr.should == val` | `assert_eq(expr, val)` | v0.1 |
| `expr.should_not == val` | `assert_neq(expr, val)` | v0.2 |
| `expr.should.foo?` | `assert(expr.foo?)` | v0.2 |
| `expr.should.foo?(args)` | `assert(expr.foo?(args))` | v0.2 |
| `expr.should_not.foo?` | `assert(!expr.foo?)` | v0.2 |
| `expr.should_not.foo?(args)` | `assert(!expr.foo?(args))` | v0.2 |
| `-> { BODY }.should.raise(CLASS)` | `assert_raises("CLASS") do BODY end` | v0.2 |
| `-> { BODY }.should.raise(M::Cls)` | `assert_raises("M::Cls") do BODY end` | v0.2 |
| `require_relative '...'` | (stripped — line filter) | v0.1 |
| *`it_behaves_like :shared, ...`* | *passthrough* | v0.3+ |
| *`should_receive` / `mock(...)`* | *passthrough* | (no mock lib in micro-runner; hand-translate) |

For the `should ==` / `should_not ==` / predicate-matcher
rewrites, `expr`, `val`, and `args` come from the original
source verbatim — regex literals, escapes, multi-line method
chains, multibyte characters, and inline blocks all preserve
their formatting exactly. The lambda-raise rewrite is the
exception: the `-> { BODY }` shape is unwrapped and the body
is re-emitted inside a `do ... end` block, so indentation
shifts (the body's own multi-statement structure stays
intact, just under different leading whitespace).

## What's deliberately deferred (v0.3+)

- **Shared examples** (`it_behaves_like :shared, ...`) — needs cross-file inlining of the shared `describe` block.
- **Mocks / `should_receive`** — micro-runner has no mock library; these always need hand-translation.
- **mspec helpers** (`mock_int(...)`, `mock(...)`, `bignum_value`, `fixnum_max`) — passthrough; needs lookup table or per-helper fixture.
- **`SpecEvaluate.desc` heredoc form** (used in `core/integer/arity_spec.rb`) — uses Ruby heredoc to embed evaluated code; not modelled.

Dropping fixtures-only `describe` blocks (`UnboundMethodSpecs::*`,
`MethodSpecs::*` etc) is also pending — those classes are
referenced inside `it` bodies and a future pass will need to
detect the fixture-only `describe` and emit inline classes.

`require_relative` lines, on the other hand, ARE stripped:
the micro-runner has no loader, so leaving them in fails the
whole file at `<file-level>`. The strip is a line-level filter
(`^\s*require_relative\b.*$`) applied after the AST rewrite,
so it runs even when prism reports parse errors and the AST
walk only partially completes.

## Parse errors and exit codes

The CLI runs prism over the source separately and prints any
parse errors to stderr before emitting the rewrite to stdout.
Output is best-effort either way — the AST walk visits
whatever sub-trees prism could build. Exit codes:

| Code | Meaning |
|---|---|
| 0 | Clean parse + rewrite done |
| 1 | Failed to read the input file |
| 2 | No path argument given |
| 3 | Parse errors found; stdout still has the partial rewrite |

The library's `extract(&str) -> String` is infallible by
design (always returns a String) so golden tests stay simple.
A separate `parse_errors(&str) -> Vec<String>` exposes the
diagnostic surface for callers that care.

## Golden tests

Two complementary test sets, both run by
`cargo test -p rubyrs-spec-extract`:

- `tests/golden/` — small hand-built fixtures that exercise
  specific code paths in isolation
  (`simple_eq`, `skipped_patterns`, `strip_require_relative`).
- `tests/upstream/` — verbatim ruby/spec snapshots
  (`core/string/empty_spec.rb`, `length_spec.rb`,
  `reverse_spec.rb` as of 2026-05). The matching
  `.expected.rb` is what the extractor produces. A separate
  `upstream_outputs_parse_as_valid_ruby` test parses every
  extracted file through prism and asserts no syntax errors,
  even when matchers v0.1 doesn't handle remain in the output.

Regenerate `.expected.rb` files after an intentional change:

```bash
UPDATE_EXPECTED=1 cargo test -p rubyrs-spec-extract
```

## Real-world result (v0.2)

Run against the three vendored fixtures:

| Upstream file | What v0.2 produces |
|---|---|
| `core/string/reverse_spec.rb` | both `describe` blocks lower fully — all `should ==`, `.should.equal?`, `.should.instance_of?(...)`, `.should.raise(FrozenError)` blocks auto-extract. Only the `MyString` subclass fixture remains as a hand-translation item (no rubyrs equivalent). |
| `core/string/empty_spec.rb` | `should.empty?` / `should_not.empty?` predicate matchers now auto-extract; the `StringSpecs::MyString.new("")` fixture line is the only hand-translation work left. |
| `core/string/length_spec.rb` | only `require_relative` stripped — the `it_behaves_like :string_length, :length` redirect is v0.3+ territory. |

v0.1 mechanised the most common shape; v0.2 closes the
predicate + raise gap. After v0.2 the typical
predicate-heavy upstream file goes from "extractor produces
a couple of lines" to "extractor produces a file that runs
end to end with minor fixture-skip polish."

What still needs a human:

- Files that use fixtures (`StringSpecs::MyString`,
  `MethodSpecs::Methods`) — extractor doesn't know which
  fixture classes exist or how to inline them.
- Files that use mspec helpers (`mock_int`, `bignum_value`).
- Files where rubyrs intentionally diverges from CRuby
  (e.g. `Math::DomainError` vs `ArgumentError`) — the
  extracted assertion runs but fails; the human polishes by
  commenting out + cross-linking `docs/SUBSET.md`.

## Approach (for future contributors)

Byte-range substitution, not AST rebuild. We parse with
`ruby_prism`, walk via the `Visit` trait looking for the
specific call shape, collect `(start, end, replacement)`
tuples per match, then apply them in reverse byte order so
earlier offsets stay valid. Whatever we don't match is left
exactly as the upstream source had it — including
whitespace, comments, and any matchers we haven't learned
yet. This minimises the "the extractor reformatted my file"
surprise and makes diffs against upstream readable.

When v0.2 lands (e.g., `should_not`), the matching logic
extends with another check inside `visit_call_node` and a
new pair of golden fixtures. No reshape of the existing
v0.1 recogniser needed.
