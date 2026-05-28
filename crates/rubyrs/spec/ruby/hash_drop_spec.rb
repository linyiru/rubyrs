# Adapted from ruby/spec core/hash/drop_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — Hash#drop
# inherits from Enumerable; behaves like Array#drop on
# `[k, v]` pair Arrays.

describe "Hash#drop" do
  it "returns the entries AFTER the first n as Array<[k, v]>" do
    h = {a: 1, b: 2, c: 3, d: 4}
    assert_eq(h.drop(1), [[:b, 2], [:c, 3], [:d, 4]])
  end

  it "returns the whole Hash (as Array<[k,v]>) when n is 0" do
    h = {a: 1, b: 2}
    assert_eq(h.drop(0), [[:a, 1], [:b, 2]])
  end

  it "returns [] when n is >= size" do
    h = {a: 1, b: 2}
    assert_eq(h.drop(2), [])
    assert_eq(h.drop(10), [])
  end

  it "returns [] on an empty Hash" do
    assert_eq({}.drop(3), [])
  end

  it "raises ArgumentError on a negative size" do
    assert_raises("ArgumentError") { {a: 1}.drop(-1) }
  end

  bignum_it "raises RangeError on a BigInt size" do
    assert_raises("RangeError") { {a: 1}.drop(10_000_000_000_000_000_000_000) }
  end
end
