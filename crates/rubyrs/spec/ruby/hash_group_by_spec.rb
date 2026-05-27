# Adapted from ruby/spec core/enumerable/group_by_spec.rb +
# core/hash/group_by_spec.rb at upstream commit 448cb340 (2026-05).
# Hand-translated — the Enumerable-shared block partitions
# pairs by the block's return value. The Enumerator no-block
# form and the size-hint variants are dropped.

describe "Hash#group_by" do
  it "returns a Hash whose values are arrays of pairs" do
    h = { a: 1, b: 2, c: 3 }
    g = h.group_by { |k, v| v.odd? }
    assert_eq(g, { true => [[:a, 1], [:c, 3]], false => [[:b, 2]] })
  end

  it "returns an empty hash when the receiver is empty" do
    assert_eq({}.group_by { |k, v| true }, {})
  end

  # skipped (method-not-implemented): it "returns an Enumerator if called without a block" do
  # skipped (method-not-implemented): it_behaves_like :enumeratorized_with_origin_size
end
