# Adapted from ruby/spec core/hash/{min,max}_spec.rb at
# upstream commit 448cb340 (2026-05). Hand-translated — the
# no-block forms compare entries via `<=>` on the `[k, v]`
# pair Array (element-wise, key first). Block forms
# (`h.min { |a, b| cmp }`) and `min_by`/`max_by` (already
# implemented) are out of subset.

describe "Hash#min" do
  it "returns the lexicographically smallest [k, v] pair" do
    h = {b: 2, a: 1, c: 3}
    # Compares pair-Arrays; [:a, 1] < [:b, 2] < [:c, 3].
    assert_eq(h.min, [:a, 1])
  end

  it "returns nil on an empty Hash" do
    assert_eq({}.min, nil)
  end

  it "uses element-wise pair comparison (key first, value tiebreaker)" do
    # Same key short-circuits to value compare. Since Hash
    # keys are unique this only matters across different
    # Hashes; verify the lexicographic order directly.
    h = {b: 1, a: 2}
    assert_eq(h.min, [:a, 2])
  end

  # skipped (method-not-implemented): it "with a block uses block as the comparator" do
  #   `h.min { |a, b| a[1] <=> b[1] }`. Block-form Enumerable
  #   comparators not modelled on Hash.
end

describe "Hash#max" do
  it "returns the lexicographically largest [k, v] pair" do
    h = {b: 2, a: 1, c: 3}
    assert_eq(h.max, [:c, 3])
  end

  it "returns nil on an empty Hash" do
    assert_eq({}.max, nil)
  end

  # skipped (method-not-implemented): it "with a block uses block as the comparator" do
end
