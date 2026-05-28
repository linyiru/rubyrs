# Adapted from ruby/spec core/hash/take_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — Hash#take
# inherits from Enumerable; behaves like Array#take but on
# `[k, v]` pair Arrays.

describe "Hash#take" do
  it "returns the first n entries as an Array of [k, v] pairs" do
    h = {a: 1, b: 2, c: 3, d: 4}
    assert_eq(h.take(2), [[:a, 1], [:b, 2]])
  end

  it "returns [] when n is 0" do
    assert_eq({a: 1}.take(0), [])
  end

  it "caps the take count at the Hash size" do
    h = {a: 1, b: 2}
    assert_eq(h.take(10), [[:a, 1], [:b, 2]])
  end

  it "returns [] on an empty Hash" do
    assert_eq({}.take(3), [])
  end

  it "raises ArgumentError on a negative size" do
    assert_raises("ArgumentError") { {a: 1}.take(-1) }
  end

  bignum_it "raises RangeError on a BigInt size" do
    assert_raises("RangeError") { {a: 1}.take(10_000_000_000_000_000_000_000) }
  end
end
