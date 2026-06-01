# Adapted from ruby/spec core/integer/to_r_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-polished:
# - `.should ==` → `assert_eq`.
# - `bignum_value` cases skipped: Phase C.4 widens Rational
#   num/den to BigInt; today `Integer#to_r` raises RangeError
#   for BigInt receivers because the canonical-form storage
#   is i64-only (see vm/dispatch.rs).

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

  # skipped (method-not-implemented): it "returns self as Rational with den 1" do
  #   bn = 2**64
  #   assert_eq(bn.to_r, Rational(bn, 1))
  # end
  # BigInt receiver. Phase C.4 widens RationalRepr's i64 num/den
  # to BigInt; today `Integer#to_r` raises RangeError for BigInt
  # magnitudes.

  # skipped (method-not-implemented): it "returns Rational(i64::MIN, 1) for the smallest fixnum" do
  #   assert_eq((-(2**62 + 2**62)).to_r, Rational(-(2**62 + 2**62), 1))
  # end
  # i64::MIN edge — Phase C.4 follow-up. `make_rational` rejects
  # i64::MIN num/den because the canonical-form sign normalization
  # would call `.abs()` on the magnitude (panics in debug for
  # `i64::MIN.abs()`). CRuby returns `Rational(-9223372036854775808, 1)`.
  # Lifts together with the BigInt receiver case above when
  # RationalRepr widens.
end
