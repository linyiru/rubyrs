# Adapted from ruby/spec core/integer/to_r_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-polished:
# - `.should ==` → `assert_eq`.
# - `bignum_value` cases gated as `bignum_it` so they only run
#   on the profile where `RationalRepr` stores BigInt num/den
#   (Phase C.4.1 widening).

describe "Integer#to_r" do
  it "returns the receiver as a Rational with denominator 1" do
    assert_eq(0.to_r, Rational(0, 1))
    assert_eq(1.to_r, Rational(1, 1))
    assert_eq(5.to_r, Rational(5, 1))
    assert_eq((-3).to_r, Rational(-3, 1))
  end

  it "returns a result whose numerator is the receiver" do
    assert_eq(5.to_r.numerator, 5)
    assert_eq((-7).to_r.numerator, -7)
  end

  it "returns a result whose denominator is 1" do
    assert_eq(5.to_r.denominator, 1)
    assert_eq(0.to_r.denominator, 1)
    assert_eq((-100).to_r.denominator, 1)
  end

  it "raises ArgumentError if passed any arguments" do
    assert_raises("ArgumentError") { 5.to_r(1) }
  end

  bignum_it "bignum: returns self as Rational with den 1" do
    bn = 2**64
    assert_eq(bn.to_r, Rational(bn, 1))
    assert_eq(bn.to_r.numerator, bn)
    assert_eq(bn.to_r.denominator, 1)
  end

  bignum_it "bignum: returns Rational(i64::MIN, 1) for the smallest fixnum" do
    # `-(2**62 + 2**62) == i64::MIN`. Pre-C.4.1 this raised
    # RangeError because `make_rational` did `.abs()` on i64::MIN
    # in debug. Phase C.4.1 widens RationalRepr to BigInt, lifting
    # the limit (paired with the BigInt receiver case above).
    n = -(2**62 + 2**62)
    assert_eq(n.to_r, Rational(n, 1))
  end
end
