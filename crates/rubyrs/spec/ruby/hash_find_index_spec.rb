# Adapted from ruby/spec core/hash/find_index_spec.rb at
# upstream commit 448cb340 (2026-05). Hand-translated —
# Hash#find_index returns the Int insertion-order index of
# the first truthy block yield, or nil if none. Yields a
# single `[k, v]` pair Array per entry.

describe "Hash#find_index" do
  it "returns the index of the first entry whose block result is truthy" do
    h = {a: 1, b: 2, c: 3}
    assert_eq(h.find_index { |k, v| v == 2 }, 1)
  end

  it "returns nil when no entry matches" do
    h = {a: 1, b: 2}
    assert_eq(h.find_index { |k, v| v > 10 }, nil)
  end

  it "returns nil on an empty Hash" do
    assert_eq({}.find_index { |x| true }, nil)
  end

  it "yields a single [k, v] Array per entry (single-param block)" do
    h = {a: 1, b: 2, c: 3}
    assert_eq(h.find_index { |pair| pair[1] == 3 }, 2)
  end

  it "short-circuits after the first truthy yield" do
    seen = []
    {a: 1, b: 2, c: 3, d: 4}.find_index { |k, v| seen << k; v == 2 }
    assert_eq(seen, [:a, :b])  # stops at :b on truthy
  end

  it "honours `break` with the break value" do
    out = {a: 1, b: 2}.find_index { |k, v| break :early }
    assert_eq(out, :early)
  end

  # skipped (method-not-implemented): it "with an argument compares each entry via ==" do
  #   CRuby's `h.find_index(target)` compares each entry
  #   `<=> target`. Out of subset — block form covers the
  #   common case.
end
