# Adapted from ruby/spec core/enumerable/max_by_spec.rb at
# upstream commit 448cb340 (2026-05). Hand-translated — the
# pair-yielding behavior on Hash receivers is inlined. The
# n-arg form (`max_by(n)`) + Enumerator no-block form are
# dropped. Mirror image of hash_min_by_spec.rb.

describe "Hash#max_by" do
  it "returns the pair with the maximum block return value" do
    h = { a: 1, b: 3, c: 2 }
    assert_eq(h.max_by { |k, v| v }, [:b, 3])
  end

  it "returns nil when the receiver is empty" do
    assert_eq({}.max_by { |k, v| v }, nil)
  end

  it "passes (key, value) into the block as two args" do
    h = { a: 1, b: 2 }
    seen = []
    h.max_by { |k, v| seen << [k, v]; v }
    assert_eq(seen, [[:a, 1], [:b, 2]])
  end

  # skipped (method-not-implemented): it "returns an Enumerator if called without a block" do
  #   The Enumerator-from-no-block surface is out of subset.
  # skipped (method-not-implemented): it "returns an array of n elements if argument is given" do
  #   The n-arg form `max_by(n)` is out of subset.
end
