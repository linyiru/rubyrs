# Adapted from ruby/spec core/hash/any_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — the no-block
# and with-block forms are inlined. The pattern-arg form
# (`h.any?(MatchPattern)`) is dropped.

describe "Hash#any?" do
  it "returns true when the hash has at least one pair" do
    assert_eq({ a: 1 }.any?, true)
    assert_eq({ a: 1, b: 2 }.any?, true)
  end

  it "returns false when the hash is empty" do
    assert_eq({}.any?, false)
  end

  it "returns true when the block is truthy for some pair" do
    assert_eq({ a: 1, b: 2 }.any? { |k, v| v > 1 }, true)
  end

  it "returns false when the block is falsy for all pairs" do
    assert_eq({ a: 1, b: 2 }.any? { |k, v| v > 5 }, false)
  end

  it "yields two args (key, value) to the block" do
    h = { a: 1, b: 2 }
    seen = []
    h.any? { |k, v| seen << [k, v]; false }
    assert_eq(seen, [[:a, 1], [:b, 2]])
  end

  # skipped (method-not-implemented): pattern-arg form `h.any?(pat)`.
end
