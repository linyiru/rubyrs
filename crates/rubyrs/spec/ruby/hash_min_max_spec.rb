# Adapted from ruby/spec core/hash/{min,max}_spec.rb at
# upstream commit 448cb340 (2026-05). Hand-translated — the
# no-block forms compare entries via `<=>` on the `[k, v]`
# pair Array (element-wise, key first). Out of subset:
# the block-form comparator (`h.min { |a, b| cmp }`).
# Note: `min_by`/`max_by` are SUPPORTED via vm/iter.rs and
# live in their own specs — they're not part of this file.

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
    # Hash key uniqueness is by `eql?`, not `<=>` — so two
    # distinct keys CAN compare Equal via `<=>` (e.g. `1`
    # and `1.0` are eql?-distinct but `1 <=> 1.0 == 0`).
    # When the key compare yields Equal, the value
    # compare kicks in as the tiebreaker.
    #
    # Here both `1` and `1.0` coexist as keys; min picks
    # the entry whose VALUE compares smaller after the
    # key tie:
    #   key:   1 <=> 1.0     → 0 (Equal)
    #   value: :first <=> :second → -1 (Less)
    # → min == [1, :first].
    h = {1 => :first, 1.0 => :second}
    assert_eq(h.min, [1, :first])
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
