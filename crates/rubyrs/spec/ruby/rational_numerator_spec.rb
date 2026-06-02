# Adapted from ruby/spec core/rational/numerator_spec.rb at
# upstream commit 448cb340 (2026-05). Hand-polished:
# - `.should ==` → `assert_eq`.
# - `bignum_value` cases gated as `bignum_it` so they only run
#   on the profile where `RationalRepr` stores BigInt num/den
#   (Phase C.4.1 widening).

describe "Rational#numerator" do
  it "returns the numerator" do
    assert_eq(Rational(3, 4).numerator, 3)
    assert_eq(Rational(-5, 7).numerator, -5)
    assert_eq(Rational(5).numerator, 5)
  end

  it "returns the reduced-form numerator after gcd normalization" do
    # gcd(6, 4) = 2 → canonical (3, 2)
    assert_eq(Rational(6, 4).numerator, 3)
    # negative-den moves sign to num
    assert_eq(Rational(3, -4).numerator, -3)
    assert_eq(Rational(-3, -4).numerator, 3)
  end

  bignum_it "bignum: returns the numerator for BigInt-magnitude num" do
    bn = 2**64
    assert_eq(Rational(bn, 3).numerator, bn)
    assert_eq(Rational(-bn, 3).numerator, -bn)
  end
end
