# Adapted from ruby/spec core/hash/first_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — Hash#first
# mirrors Array#first: no-arg returns the first entry as a
# `[k, v]` pair Array (or nil on empty); arg-form returns
# the first n entries as Array<[k, v]> (capped at size,
# negative raises ArgumentError).

describe "Hash#first" do
  it "returns the first entry as a [k, v] pair Array" do
    h = {a: 1, b: 2, c: 3}
    assert_eq(h.first, [:a, 1])
  end

  it "returns nil on an empty Hash" do
    assert_eq({}.first, nil)
  end

  it "with an Int arg returns the first n entries" do
    h = {a: 1, b: 2, c: 3}
    assert_eq(h.first(2), [[:a, 1], [:b, 2]])
  end

  it "caps the take count at the Hash size" do
    h = {a: 1, b: 2}
    assert_eq(h.first(10), [[:a, 1], [:b, 2]])
  end

  it "returns an empty Array on an empty Hash with an Int arg" do
    assert_eq({}.first(3), [])
  end

  it "raises ArgumentError on a negative size" do
    assert_raises("ArgumentError") { {a: 1}.first(-1) }
  end

  bignum_it "raises RangeError on a BigInt size (cannot fit i64)" do
    # Mirrors Array#first(BigInt) — a take-count larger
    # than i64::MAX can never be a meaningful collection
    # size, so we raise rather than silently saturate.
    assert_raises("RangeError") { {a: 1}.first(10_000_000_000_000_000_000_000) }
  end
end
