# Adapted from ruby/spec core/hash/count_spec.rb +
# core/enumerable/count_spec.rb at upstream commit 448cb340
# (2026-05). Hand-translated — no-arg returns size; with-block
# counts truthy results. Pattern-arg form is dropped.

describe "Hash#count" do
  it "returns the number of pairs when called without a block or arg" do
    assert_eq({}.count, 0)
    assert_eq({ a: 1 }.count, 1)
    assert_eq({ a: 1, b: 2, c: 3 }.count, 3)
  end

  it "counts pairs for which the block returns truthy" do
    h = { a: 1, b: 2, c: 3 }
    assert_eq(h.count { |k, v| v.odd? }, 2)
    assert_eq(h.count { |k, v| v > 10 }, 0)
  end

  it "yields two args (key, value) to the block" do
    h = { a: 1, b: 2 }
    seen = []
    h.count { |k, v| seen << [k, v]; false }
    assert_eq(seen, [[:a, 1], [:b, 2]])
  end

  # skipped (method-not-implemented): pattern-arg form `h.count(pat)`.
end
