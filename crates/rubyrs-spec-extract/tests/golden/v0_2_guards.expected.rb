# Guards added in response to the /code-review pass on
# v0.2. Each block exercises one of the four review-driven
# defensive cases. None of these patterns should be rewritten
# by the extractor — they all fall through to passthrough so
# a human can finish or so the wrong rewrite never appears.

describe "guards" do
  # Cluster A — chained predicate. Outer .frozen? has a non-
  # `should*` receiver. v0.2 used to recurse into the inner
  # `arr.should.first` and rewrite it as `assert(arr.first)`,
  # leaving `.frozen?` chained off the assert's return —
  # producing `assert(arr.first).frozen?`. Now the extractor
  # doesn't recurse into outer-call receivers; the whole line
  # passes through.
  it "chained predicate" do
    arr.should.first.frozen?
  end

  # Cluster B — non-constant class arg to raise. The old
  # extractor would slice `some_var.class` literally and emit
  # `assert_raises("some_var.class")` — never matches.
  it "non-constant class arg" do
    -> { x }.should.raise(some_var.class)
  end

  # Cluster B (second form) — string-literal class arg. Old
  # extractor would include the quotes: `assert_raises("\"X\"")`.
  it "string-literal class arg" do
    -> { x }.should.raise("FrozenError")
  end

  # Cluster C — non-predicate name on the should chain. Real
  # mspec has no `should.first` matcher; the chain was being
  # silently rewritten as `assert(foo.first)`. Now requires
  # `?` suffix on the method name.
  it "non-predicate method on should chain" do
    foo.should.first
  end

  # Block-form predicate (Copilot review on the v0.2 guards
  # commit). `.should.all? { ... }` would, under naive
  # rewrite, become `assert(arr.all? { |x| x > 0 })` — the
  # `{ }` block is inside `assert`'s parens so binds to
  # `all?`, which is correct. But the `do/end` form has
  # lower precedence and could bind to `assert` instead.
  # Safer: passthrough any predicate call that has a block.
  it "predicate with brace block" do
    arr.should.all? { |x| x > 0 }
  end

  it "predicate with do/end block" do
    arr.should.all? do |x|
      x > 0
    end
  end

  # Cluster D — parenthesised low-precedence receiver. If
  # prism's `receiver()` preserves the ParenthesesNode, the
  # slice keeps the parens and the rewrite is correct. If it
  # unwraps to the inner OrNode, parens get lost and
  # precedence breaks. The expected output below pins which
  # behaviour prism actually has; a regression in prism would
  # surface as a golden diff.
  it "parenthesised receiver in predicate matcher" do
    assert((a || b).empty?)
  end
end

# Sanity — the patterns we DO rewrite still rewrite. Pins the
# v0.2 happy path inside the same fixture so refactors to the
# recogniser order don't quietly break it.
describe "happy paths still work" do
  it "should ==" do
    assert_eq("x".length, 1)
  end

  it "should.PRED? (real predicate)" do
    assert([].empty?)
  end

  it "lambda raise with constant arg" do
    assert_raises("ArgumentError") do
      x
    end
  end
end
