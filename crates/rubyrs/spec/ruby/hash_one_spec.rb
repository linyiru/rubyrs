# Adapted from ruby/spec core/hash/one_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — completes
# the Enumerable any?/all?/none?/one? quad for Hash.

describe "Hash#one?" do
  it "without a block returns true iff the Hash has exactly one entry" do
    # Every Hash entry is a truthy `[k, v]` pair, so the
    # no-block Enumerable shape collapses to a size check.
    assert_eq({}.one?, false)
    assert_eq({a: 1}.one?, true)
    assert_eq({a: 1, b: 2}.one?, false)
  end

  it "returns true iff exactly one entry yields truthy" do
    h = {a: 1, b: 2, c: 3}
    assert_eq(h.one? { |k, v| v == 2 }, true)
    assert_eq(h.one? { |k, v| v > 1 }, false)
  end

  it "returns false on an empty Hash" do
    assert_eq({}.one? { |k, v| true }, false)
  end

  it "returns false when no entry matches" do
    assert_eq({a: 1, b: 2}.one? { |k, v| v > 10 }, false)
  end

  it "short-circuits after a second truthy yield" do
    # Once two truthies are seen, the answer is fixed at
    # false. Block should not be invoked for remaining
    # entries. The implementation increments a counter on
    # each truthy yield and breaks the moment `count > 1`,
    # so for an all-truthy predicate over `[1, 2, 3, 4]`
    # the block runs EXACTLY twice (iter 1 sets count=1,
    # iter 2 sets count=2 and breaks before the rest).
    h = {a: 1, b: 2, c: 3, d: 4}
    count = 0
    h.one? { |k, v| count = count + 1; v >= 1 }
    assert_eq(count, 2)
  end
end
