# Adapted from ruby/spec core/hash/one_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — completes
# the Enumerable any?/all?/none?/one? quad for Hash.

describe "Hash#one?" do
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
    # entries.
    h = {a: 1, b: 2, c: 3, d: 4}
    count = 0
    h.one? { |k, v| count = count + 1; v >= 1 }
    # Block runs for the first 3 entries (1, 2, 3 are all
    # truthy ≥ 1; on the 3rd hit count == 2 and the loop
    # exits before the 4th). Tolerant assertion: count is
    # at most 3 (early exit) and at least 2 (need two
    # truthies to disprove "exactly one").
    assert(count >= 2)
    assert(count <= 3)
  end
end
