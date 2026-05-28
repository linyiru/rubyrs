# Adapted from ruby/spec core/integer/floor_spec.rb +
# shared/to_i.rb + shared/integer_rounding.rb +
# shared/integer_floor_precision.rb at upstream commit 448cb340.
# Same hand-polish conventions as integer_ceil_spec.rb (sibling).

describe "Integer#floor" do
  it "fixnum: returns self for to_i shape (no precision)" do
    assert_eq(10.floor, 10)
    assert_eq((-15).floor, -15)
  end

  bignum_it "bignum: returns self" do
    bn = 2**64
    assert_eq(bn.floor, bn)
    assert_eq((-bn).floor, -bn)
  end

  it "returns self if not passed a precision" do
    [2, -4].each { |v| assert_eq(v.floor, v) }
  end

  it "returns self if passed a precision of zero" do
    [2, -4].each { |v| assert_eq(v.floor(0), v) }
  end

  it "returns itself if passed a positive precision" do
    [2, -4].each { |v| assert_eq(v.floor(42), v) }
  end

  it "precision is zero: returns integer self" do
    assert_eq(0.floor(0), 0)
    assert_eq(123.floor(0), 123)
    assert_eq((-123).floor(0), -123)
  end

  it "precision is positive: returns self" do
    assert_eq(0.floor(1), 0)
    assert_eq(0.floor(10), 0)
    assert_eq(123.floor(10), 123)
    assert_eq((-123).floor(10), -123)
  end

  it "precision is negative: always returns 0 when self is 0" do
    assert_eq(0.floor(-1), 0)
    assert_eq(0.floor(-10), 0)
  end

  it "precision is negative: returns largest integer <= self with trailing zeros" do
    assert_eq(123.floor(-1), 120)
    assert_eq(123.floor(-2), 100)
    assert_eq(123.floor(-3), 0)
    assert_eq((-123).floor(-1), -130)
    assert_eq((-123).floor(-2), -200)
    assert_eq((-123).floor(-3), -1000)
  end

  # skipped (method-not-implemented): precision -20 / -50 needs
  # BigInt-aware rounding — see integer_ceil_spec.rb header.
end
