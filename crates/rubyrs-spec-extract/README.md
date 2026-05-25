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

## What v0.1 recognises

Exactly this shape:

```ruby
expr.should == val
# →
assert_eq(expr, val)
```

`expr` and `val` are taken verbatim from the source, so
regex literals, escapes, multi-line method chains, and inline
blocks all preserve their original formatting.

## What v0.1 deliberately doesn't do

Each of these passes through verbatim, so a human reviewer
can see what's still hand-translation territory:

| Upstream pattern | What's needed to recognise |
|---|---|
| `expr.should_not == val` | `assert_neq` helper in `spec_helper.rb` |
| `expr.should.foo?` (predicate matcher) | Per-predicate knowledge or a generic `assert(expr.foo?)` |
| `-> { ... }.should.raise(X)` | Parse the lambda + matcher class; lower to `assert_raises("X") { ... }` |
| `it_behaves_like :shared, ...` | Inline shared examples; needs cross-file resolution |
| `should_receive` / mocks | We have no mock library — skip and hand-translate |

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

## v0.1 real-world result

Run against the three vendored fixtures:

| Upstream file | What v0.1 produces |
|---|---|
| `core/string/reverse_spec.rb` | 6 of ~10 `it` blocks lower cleanly to `assert_eq` calls; predicate / lambda / `should.equal?` blocks pass through unchanged |
| `core/string/empty_spec.rb` | only `require_relative` lines stripped — file body is all predicate matchers, nothing to rewrite yet |
| `core/string/length_spec.rb` | only `require_relative` lines stripped — the single `it_behaves_like` redirect is untouched |

In other words, v0.1 is a STARTER that mechanises the most
common pattern. Files that mix matchers still need a human
finish; the extracted file is the right starting point for
that finish. v0.2 (`should_not`, predicate matchers,
`should.raise`) will close the gap for the predicate-heavy
files; v0.4 (shared examples) for the `it_behaves_like` ones.

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
