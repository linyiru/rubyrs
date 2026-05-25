# `spec/` — ruby/spec micro-runner

A minimal runner for ruby/spec-flavoured tests that exercises
rubyrs's metaprogramming features against the same shape of
specifications CRuby / JRuby / TruffleRuby use.

This is **not** a port of MSpec. The real MSpec depends on
CRuby internals (`Kernel#load`, anonymous `Class.new { ... }`,
`.should ==` operator-method-missing chains, mock libraries)
that are well outside rubyrs's subset. Instead `spec_helper.rb`
provides function-style matchers and each spec file uses only
the subset features we already ship.

## Layout

```
spec/
├── README.md          ← this file
├── spec_helper.rb     ← describe / it / assert_eq / assert_raises
└── ruby/
    ├── alias_method_spec.rb       # metaprog set (ADR 0010)
    ├── class_eval_spec.rb
    ├── define_method_spec.rb
    ├── instance_eval_spec.rb
    ├── method_missing_spec.rb
    ├── singleton_method_spec.rb
    │
    ├── string_sub_spec.rb         # core/string subset (manually
    ├── string_gsub_spec.rb        # translated from upstream
    ├── string_reverse_spec.rb     # ruby/spec snapshot ~2026-05;
    ├── string_include_spec.rb     # see TESTING.md for the
    ├── string_empty_spec.rb       # ingestion-pipeline roadmap)
    │
    │ # core/method subset — each file mirrors the same-named
    │ # upstream spec; surfaces covered are `Method#call` / `#()`,
    │ # `#<<` / `#>>` composition, `#curry`, `#==`, `#owner`,
    │ # `#receiver`, `#to_proc` (explicit + via `&`).
    ├── method_call_spec.rb
    ├── method_compose_spec.rb
    ├── method_curry_spec.rb
    ├── method_equal_spec.rb
    ├── method_owner_spec.rb
    ├── method_receiver_spec.rb
    ├── method_to_proc_spec.rb
    │
    └── unbound_method_equal_spec.rb # core/unboundmethod subset
```

The runner is at
[`crates/rubyrs/tests/ruby_spec.rs`](../tests/ruby_spec.rs) and
runs as part of `cargo test -p rubyrs`. Every example must pass
— there's no "tag this as known-divergent" mechanism yet (see
"Future work" below). Current total: **108 examples across 19
files**, all passing.

## DSL the helper provides

```ruby
describe "Module#alias_method" do
  it "adds a new name for an existing method" do
    class Greeter
      def hello; "hi"; end
      alias_method :greet, :hello
    end
    assert_eq(Greeter.new.greet, "hi")
  end

  it "raises NameError on missing source" do
    assert_raises("NameError") do
      class Bad
        alias_method :a, :nonexistent
      end
    end
  end
end
```

Matchers:

| Helper | Pass condition |
|---|---|
| `assert(cond, label = "assert")` | `cond` is truthy |
| `assert_eq(actual, expected)` | `actual == expected` (Ruby `==`) |
| `assert_raises(class_name) { ... }` | block raises a class whose `e.class.to_s` matches `class_name` |

What we deliberately don't provide:

- **`.should ==`** — would need method_missing on every Value to dispatch matcher symbols. Use `assert_eq` instead.
- **`Class.new { ... }`** anonymous classes — every spec defines named classes at toplevel. The runner gives each file a fresh Runtime so names don't collide across files (but they do collide within a file — adopt distinct names per `it`).
- **`before` / `after` hooks** — each `it` block is fully self-contained.
- **Skip / pending tags** — a feature lands as expected-pass or doesn't land at all in `spec/`.

## How CI sees a spec

The runner registers five `__spec_*` host functions which
the Ruby-side helpers call to report:

- `__spec_describe_push(name)` — enter a describe scope
- `__spec_describe_pop` — leave the current describe (driven
  from spec_helper's `begin / ensure` around `yield`, so
  nested + raising blocks restore correctly)
- `__spec_it(name)` — start a new example
- `__spec_pass(label)` — record an assertion success
- `__spec_fail(message)` — record an assertion failure

Each `it` block is considered passing only when **at least
one** matcher reported a pass AND **zero** matchers reported a
fail. An `it` block that never calls a matcher (and doesn't
raise) is treated as failing — prevents silently-empty
examples from looking green. A pass or fail reported outside
any `it` (e.g. `assert_eq` at describe scope) lands on a
synthetic `<orphan>` example so the misuse is loud.

If a spec file itself fails to parse or raises an exception
outside any `it` block, the runner synthesises a `<file-level>`
example so the failure shows up in the report rather than
vanishing into a zero-example file.

## Translation conventions (for the `core/string_*_spec.rb` set)

The metaprog specs were written from scratch against rubyrs's
documented behaviour. The `string_*_spec.rb` files are
manually-translated subsets of the corresponding upstream
ruby/spec files (e.g. `string_sub_spec.rb` ← upstream
`core/string/sub_spec.rb`). The translation rules are
deliberately mechanical so a future `tools/spec_extract`
(see [`docs/TESTING.md`](../../../docs/TESTING.md) — the
Layer-4 pipeline) can automate the same lift.

Conversions applied:

| Upstream form | Translated to |
|---|---|
| `expr.should == val` | `assert_eq(expr, val)` |
| `expr.should_not.equal?(other)` | `assert_eq(expr.equal?(other), false)` |
| `-> { ... }.should.raise(ExceptionClass)` | `assert_raises("ExceptionClass") { ... }` |
| `expr.should.empty?` (predicate matcher) | `assert_eq(expr.empty?, true)` |
| `expr.should_not.empty?` | `assert_eq(expr.empty?, false)` |
| `it_behaves_like :shared, ...` | inlined or skipped (shared specs not vendored) |

Whole-block skips per file are noted at the top of each spec
file with the reason — usually one of:

- **Out of subset** — `force_encoding`, `Class.new { ... }`,
  mock objects, multibyte / broken encodings, `to_str` coercion
  protocol via mocks.
- **Out of master** — a feature rubyrs hasn't shipped yet
  (e.g. `/i` case-insensitive flag on Regex, `\1`/`\&`
  backref replacement strings).
- **Subclass / fixtures** — upstream's `StringSpecs::MyString`
  fixtures aren't vendored; tests that check subclass identity
  are dropped.

Don't smuggle a divergence into the spec — write the spec to
match upstream behaviour. If rubyrs differs intentionally,
document the divergence in
[`docs/SUBSET.md`](../../../docs/SUBSET.md) and skip the spec
case with a `#` comment naming the upstream source line.

## Adding a new spec

1. Pick the upstream ruby/spec file you want to mirror (e.g.,
   `core/module/define_method_spec.rb`).
2. Copy or write the `describe` / `it` blocks into a new
   `spec/ruby/<feature>_spec.rb`. Trim anything outside our
   subset: rest-arg in block params, `Class.new { ... }`,
   `instance_method`, mock objects.
3. Use named classes inside `it` blocks (`class MyTest1; ...; end`).
4. Run `cargo test -p rubyrs --test ruby_spec` and iterate.
5. For intentional rubyrs/CRuby divergences, follow the
   convention from the "Translation conventions" section above:
   document the divergence in `docs/SUBSET.md` and skip the
   upstream `it` block with a `#` comment naming the
   upstream source line — do NOT rewrite the assertion to
   match rubyrs's narrower behaviour. Earlier versions of this
   README told contributors to do the rewrite; that was wrong
   for a spec set whose purpose is to mirror upstream.

## What this gates against (vs. `tests/embed.rs`)

`embed.rs` locks rubyrs's **embedding API** (Runtime / register_fn /
Value shapes). Spec coverage locks the **language semantics**
against a third-party oracle. The two complement: a metaprog
feature can have an embed test that exercises rubyrs's public
surface and a spec test that exercises CRuby-compatible
behaviour. When they disagree, SUBSET.md decides which is
right.

## Future work

- **Pass / divergence tags**: when we want to vendor real
  upstream spec files unmodified (vs. our curated adaptations),
  we'll need a `tags/` file marking which examples are expected
  to pass vs. known-divergent vs. blocked-by-missing-feature.
  Modelled on TruffleRuby's tagged-spec workflow.
- **Snapshot refresh**: periodically rebase the curated files
  against ruby/spec's latest. Currently sourced from upstream
  ~2026-05.
- **Per-spec output capture**: spec helpers like `puts "debug"`
  inside an `it` block currently land on the real stdout; could
  redirect into the example's outcome for cleaner CI logs.
