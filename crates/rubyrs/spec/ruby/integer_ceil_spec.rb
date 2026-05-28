# Adapted from ruby/spec core/integer/ceil_spec.rb +
# shared/to_i.rb + shared/integer_rounding.rb +
# shared/integer_ceil_precision.rb at upstream commit 448cb340
# (2026-05). Hand-translated — upstream uses
# `it_behaves_like :integer_to_i, :ceil` against shared/to_i.rb;
# inline the runnable cases.
#
# Hand-polish:
# - `.should.eql?(x)` → `assert_eq(actual, x)` (the micro-runner's
#   assert_eq uses ruby_eq, not eql?, but for Integer × Integer
#   the difference is moot — no cross-class comparisons here).
# - `bignum_value` → `(2**64)`; bignum cases gated on `bignum_it`.
# - skipped (method-not-implemented): the `10**70` / `10**100` /
#   precision -20 / -50 cases need BigInt-aware rounding;
#   numeric.rs declines past |n| == 18 and bignum_primitive
#   doesn't yet implement these selectors. Tracked as follow-up.

describe "Integer#ceil" do
  it "fixnum: returns self for to_i shape (no precision)" do
    assert_eq(10.ceil, 10)
    assert_eq((-15).ceil, -15)
  end

  bignum_it "bignum: returns self" do
    bn = 2**64
    assert_eq(bn.ceil, bn)
    assert_eq((-bn).ceil, -bn)
  end

  it "returns self if not passed a precision" do
    [2, -4].each { |v| assert_eq(v.ceil, v) }
  end

  it "returns self if passed a precision of zero" do
    [2, -4].each { |v| assert_eq(v.ceil(0), v) }
  end

  it "returns itself if passed a positive precision" do
    [2, -4].each { |v| assert_eq(v.ceil(42), v) }
  end

  it "precision is zero: returns Integer equal to self" do
    assert_eq(0.ceil(0), 0)
    assert_eq(123.ceil(0), 123)
    assert_eq((-123).ceil(0), -123)
  end

  it "precision is positive: returns self" do
    assert_eq(0.ceil(1), 0)
    assert_eq(0.ceil(10), 0)
    assert_eq(123.ceil(10), 123)
    assert_eq((-123).ceil(10), -123)
  end

  it "precision is negative: always returns 0 when self is 0" do
    assert_eq(0.ceil(-1), 0)
    assert_eq(0.ceil(-10), 0)
  end

  it "precision is negative: returns Integer equal to self if precision.abs trailing zeros" do
    assert_eq(10.ceil(-1), 10)
    assert_eq(100.ceil(-1), 100)
    assert_eq(100.ceil(-2), 100)
    assert_eq((-10).ceil(-1), -10)
    assert_eq((-100).ceil(-1), -100)
    assert_eq((-100).ceil(-2), -100)
  end

  it "precision is negative: returns smallest Integer >= self with trailing zeros" do
    assert_eq(123.ceil(-1), 130)
    assert_eq(123.ceil(-2), 200)
    assert_eq(123.ceil(-3), 1000)
    assert_eq((-123).ceil(-1), -120)
    assert_eq((-123).ceil(-2), -100)
    assert_eq((-123).ceil(-3), 0)
    assert_eq(100.ceil(-3), 1000)
    assert_eq((-100).ceil(-3), 0)
  end

  # skipped (method-not-implemented): precision -20 / -50 needs
  # BigInt-aware rounding (10^20 overflows i64). numeric.rs
  # declines past |n| == 18; bignum_primitive doesn't yet
  # implement these selectors.
end
