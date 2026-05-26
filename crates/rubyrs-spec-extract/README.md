# `rubyrs-spec-extract`

Mechanically rewrites upstream [ruby/spec](https://github.com/ruby/spec)
files into the `assert_eq` / `assert_raises` shape that rubyrs's
micro-runner consumes (`crates/rubyrs/spec/`).

This crate implements Layer 4 of the testing strategy
([`docs/TESTING.md`](https://github.com/linyiru/rubyrs/blob/master/docs/TESTING.md)).
The current release is v0.4 — see the "What the extractor
recognises" table below for the exact pattern set, and
"Known limitations" for the documented trade-offs.
The hand-translated specs in
[`crates/rubyrs/spec/ruby/`](https://github.com/linyiru/rubyrs/tree/master/crates/rubyrs/spec/ruby)
(from PRs #48 / #52 / #55) remain the reference shape;
running the extractor against the same upstream sources
should reproduce that shape for the patterns the extractor
recognises, leaving the rest as passthrough for a human to
polish.

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

For batches that touch core classes (Array, String, Integer)
the extractor output is usually one polish step away from
landing. The companion `scripts/polish.py` removes:

  - **`it` blocks** whose body matches a `DROP_PATTERNS` entry —
    fixtures (`ArraySpecs.recursive_array`, `MyArray[...]`),
    unimplemented method FORMS (`Array#min { ... }` block-
    comparator, count-form `Array#first(n)`, multi-arg
    `Array#push(a, b, c)` — single-arg `.push(x)` is fine), and
    `mock`/`should_receive`. (Frozen-state behavior is
    type-specific: rubyrs implements `FrozenError` raising for
    `String` but not `Array`/`Hash`. Array-frozen specs always
    use the `ArraySpecs.frozen_array` fixture and get caught by
    the fixture rule, so polish doesn't need a separate
    FrozenError pattern that would over-drop String specs.)
  - **Top-level `before`/`after` hook blocks** the extractor's
    v0.3 `before :each` lifter didn't pick up (multi-arg,
    non-flat context, `before :all`, `after :each`) — these
    would otherwise file-level-trap with `undefined method
    \`before\` for NilClass`.

Each drop leaves a `# skipped (<category>): ...` trace at the
original block's indentation. Categories: `fixture` /
`mock` / `method-not-implemented` for `it`
blocks; `before-not-lifted` / `after-not-supported` for hook
blocks. `git grep "# skipped (method-not-implemented)"` finds
every block that would unlock when one feature PR lands.

Pipeline shape:

```bash
cargo run --release -p rubyrs-spec-extract \
  -- /path/to/ruby-spec/core/array/length_spec.rb \
  --shared /path/to/ruby-spec/core/array/shared/length.rb \
  | crates/rubyrs-spec-extract/scripts/polish.py \
  > crates/rubyrs/spec/ruby/array_length_spec.rb
```

Adding a new pattern to drop is one regex line in `polish.py`'s
`DROP_PATTERNS`; the `# skipped` comments make future revisits
(e.g., when multi-arg `Array#push` lands as a feature) easy to
find and re-evaluate.

## What the extractor recognises (current: v0.4)

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
| `before :each do BODY end` inside describe | BODY lifted into each sibling `it`; `before` call deleted | **v0.3** |
| `mock_int(LITERAL_INT)` | `LITERAL_INT` | **v0.3** |
| Patterns the extractor recognises but doesn't rewrite (curated allow-list — see below) | passthrough + listed in **skip-log header** comment at top of file | **v0.3** |
| `require_relative '...'` | (stripped — line filter) | v0.1 |
| *`after :each / :all`* | *passthrough + skip-log entry* | logged v0.3; full rewrite is future work, not yet scheduled |
| *`before :all`* | *passthrough + skip-log entry* | logged v0.3; full rewrite is future work, not yet scheduled |
| *`context "..." do ... end`* | *passthrough + skip-log entry* | logged v0.3; the micro-runner doesn't define `context` — must be renamed to `describe` (or removed) during hand-polish, otherwise the file fails at load with NoMethodError on `context` |
| `it_behaves_like :NAME, args...` (with matching `--shared <path/to/shared.rb>`) | shared body inlined at call site; `@method` / `@method2` / ... substituted with the positional args; recognisers run on the substituted body | **v0.4** |
| *`it_behaves_like :NAME, ...` (no matching shared file)* | *passthrough + skip-log entry naming the missing `--shared`* | v0.4 (logging-only when shared file not supplied) |
| *`should_receive` / `mock(...)` / `mock_int(dynamic)`* | *passthrough + skip-log entry* | logged v0.3; no mock lib in micro-runner — always hand-translate |

The skip-log header lists a **curated allow-list** of names —
not every unrewritten call. Current entries:

- `before` (when not v0.3-liftable: `before :all`, multi-arg
  `before :each, :foo`, before inside a non-flat context)
- `after`
- `context`
- `it_behaves_like`
- `mock` / `mock_int` (when not v0.3-substitutable) /
  `should_receive`

Adding a new entry is a one-line match arm in
`UnhandledCollector::visit_call_node`'s `detail` switch. The
allow-list approach keeps the header focused on patterns we
can actually advise on, instead of a wall of every Ruby call
the extractor saw.

For the `should ==` / `should_not ==` / predicate-matcher
rewrites, `expr`, `val`, and `args` come from the original
source verbatim — regex literals, escapes, multi-line method
chains, multibyte characters, and inline blocks all preserve
their formatting exactly. The lambda-raise rewrite is the
exception: the `-> { BODY }` shape is unwrapped and the body
is re-emitted inside a `do ... end` block, so indentation
shifts (the body's own multi-statement structure stays
intact, just under different leading whitespace).

## v0.4 highlights

Shared-example inlining. For files that use
`it_behaves_like :NAME, args...`, pass each `shared/*.rb`
upstream file via `--shared <path>` (repeatable):

```bash
cargo run --release -p rubyrs-spec-extract -- \
  /path/to/upstream/core/string/length_spec.rb \
  --shared /path/to/upstream/core/string/shared/length.rb
```

The extractor:

1. Parses each `--shared` file, looking for
   `describe :NAME, shared: true do BODY end` blocks.
   Builds a `name → body source` registry.
2. In the consumer, finds `it_behaves_like :NAME, arg1,
   arg2, ...` calls. For each, looks up `:NAME` in the
   registry; substitutes `@method` (= `arg1`), `@method2`
   (= `arg2`), etc. inside a fresh copy of the shared body;
   runs the matcher recognisers (so `should ==` inside the
   shared body becomes `assert_eq(...)`); replaces the
   `it_behaves_like` call with the unwrapped, rewritten
   body.
3. Unknown `:NAME`s (shared file not supplied) fall through
   to the skip-log header.

Multi-arg shared examples (`it_behaves_like :foo, :method,
:other`) work — each positional arg maps to `@method`,
`@method2`, etc. by index.

Substitution is plain text replace on the body slice
because mspec uses bare `@method` identifiers, never as
prefixes of longer names like `@methodology`. Multi-arg
forms substitute highest-index first (`@method2` /
`@method3` / ... before bare `@method`) so the `@method`
prefix of higher-numbered placeholders doesn't get rewritten
out from under them.

### Known limitation: `before :each` doesn't cover inlined `it`s

A consumer like

```ruby
describe "Foo" do
  before :each do
    @ctx = 1
  end
  it_behaves_like :foo_specs, :method
end
```

… should (per mspec semantics) run `@ctx = 1` before each
inlined `it` block from the shared body. v0.4 doesn't do
this: the lifter operates on the original AST where
`it_behaves_like` is a single call, not the inlined `it`s
it will become. Properly fixing this needs a two-pass
extract (inline → re-parse → lift) and is deferred to a
future release. In practice the combination is rare in
upstream files; when it appears, hand-inline the `before`
body into each inlined `it` block as part of the polish.

## v0.3 highlights

Three additions on top of v0.2's matcher set:

1. **`before :each` body lift** — `before :each do BODY end`
   nested directly inside a `describe ... do ... end` block
   has its BODY inserted at the start of each sibling `it`
   block, and the `before` call itself is deleted from the
   output. Only handles the flat case (`before :each` + `it`
   as direct siblings). `before :all`, `after :*`, and
   `before :each` inside a nested `context` pass through and
   land in the skip-log header.

2. **`mock_int(LITERAL_INT)` substitution** — `mock_int(2)`
   becomes `2`. Mspec's `mock_int` wraps an Integer in a fake
   `to_int` responder; for the micro-runner the literal is
   the same effective value, so we save a hand-edit. Only
   the single Integer-literal arg form is substituted; any
   variable or multi-arg form passes through and is logged.

3. **Skip-log header comment** — when the output still
   contains patterns the extractor didn't rewrite (every
   `after`, `it_behaves_like`, `should_receive`, etc.), the
   extractor prepends a comment block at the top listing
   each pattern's line number and what it'd take to handle
   it. A human polish step now has a checklist instead of
   needing to grep the output.

Caveat — `mock_int` nested inside a `should ==` doesn't
substitute (the recogniser's outer match consumes the full
range; v0.2's documented Cluster E limitation). For the
inside-`should ==` case the human polish step still applies.

## Known limitations

These aren't bugs — they're deliberate trade-offs the
`/code-review` pass surfaced and we documented rather than
fixed in v0.2. Each is single-PR-shaped follow-up work.

1. **Receiver chains are not recursed into.** When the
   extractor visits an outer CallNode that doesn't match any
   recogniser, it walks the call's arguments and block but
   NOT its receiver. Rewriting inside a receiver chain would
   orphan the outer call: source like `arr.should.first.frozen?`
   would otherwise become `assert(arr.first).frozen?`, where
   the `.frozen?` chains off the assert's return (Nil)
   instead of the original `arr.should.first` value. The
   safer rule is "leave the whole chain alone." Cost: a
   `should ==` or predicate matcher buried INSIDE another
   call's receiver chain is no longer rewritten — but those
   shapes don't appear in real upstream specs.

2. **Class arg to `should.raise` must be a constant.** Only
   `ConstantReadNode` (`ArgumentError`) and `ConstantPathNode`
   (`Math::DomainError`) are accepted. String-literal arguments
   (`should.raise("FrozenError")`) or dynamic ones
   (`should.raise(some_var.class)`) fall through to passthrough.
   Otherwise the extractor would emit `assert_raises("<text>")`
   with the source slice verbatim, which never matches
   `e.class.to_s` at runtime (always-failing test).

3. **Predicate matcher requires a `?` suffix.** Only methods
   whose name ends in `?` are eligible for the
   `.should.PRED?` → `assert(lhs.PRED?)` rewrite. Mspec's
   predicate-matcher convention is `?`-suffixed; non-`?`
   forms (`.should.first`, `.should.size`) aren't matchers
   and would be silently wrapped in an `assert(...)` that
   evaluates truthiness incorrectly. Real upstream doesn't
   use them, but the gate is defensive.

4. **Nested-args rewriting is not chained.** A pattern
   buried in the argument list of a matched outer call —
   e.g. `arr.should.include?(other.should == 3)` — is NOT
   recursively rewritten today. The outer match consumes the
   whole subtree and we substitute it; the inner `should ==`
   stays as-is in the substituted text. The downstream output
   has a leftover `should` call that the micro-runner can't
   resolve. This is a real limitation rather than a guard;
   a future PR could recursively `extract()` argument text
   before splicing it into the replacement. Not common in
   upstream `core/*` specs (nested `should` is unusual style).

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
