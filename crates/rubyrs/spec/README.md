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
    ├── alias_method_spec.rb
    ├── define_method_spec.rb
    └── method_missing_spec.rb
```

The runner is at
[`crates/rubyrs/tests/ruby_spec.rs`](../tests/ruby_spec.rs) and
runs as part of `cargo test -p rubyrs`. Every example must pass
— there's no "tag this as known-divergent" mechanism yet (see
"Future work" below).

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

The runner registers four `__spec_*` host functions
(`__spec_describe`, `__spec_it`, `__spec_pass`, `__spec_fail`)
which Ruby-side helpers call to report. Each `it` block is
considered passing only when **at least one** matcher reported a
pass AND **zero** matchers reported a fail. An `it` block that
never calls a matcher (and doesn't raise) is treated as failing
— prevents silently-empty examples from looking green.

If a spec file itself fails to parse or raises an exception
outside any `it` block, the runner synthesises a `<file-level>`
example so the failure shows up in the report rather than
vanishing into a zero-example file.

## Adding a new spec

1. Pick the upstream ruby/spec file you want to mirror (e.g.,
   `core/module/define_method_spec.rb`).
2. Copy or write the `describe` / `it` blocks into a new
   `spec/ruby/<feature>_spec.rb`. Trim anything outside our
   subset: rest-arg in block params, `Class.new { ... }`,
   `instance_method`, mock objects.
3. Use named classes inside `it` blocks (`class MyTest1; ...; end`).
4. Run `cargo test -p rubyrs --test ruby_spec` and iterate.

When you find a divergence that's intentional (documented in
SUBSET.md), don't smuggle it into the spec — write the spec to
match rubyrs's documented behaviour and add a comment pointing
at SUBSET.md.

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
