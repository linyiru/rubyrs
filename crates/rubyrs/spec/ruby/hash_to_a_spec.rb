# Adapted from ruby/spec core/hash/to_a_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — first block
# fits the micro-runner surface (each_pair + to_a). The second
# block uses `Hash#entries` (Enumerable alias not in subset).

describe "Hash#to_a" do
  it "returns a list of [key, value] pairs with same order as each()" do
    h = { a: 1, 1 => :a, 3 => :b, b: 5 }
    pairs = []

    h.each_pair do |key, value|
      pairs << [key, value]
    end

    assert_eq(h.to_a.is_a?(Array), true)
    assert_eq(h.to_a, pairs)
  end

  # skipped (method-not-implemented): it "is called for Enumerable#entries" do
  #   Uses `Hash#entries` — Enumerable surface not in subset.
end
