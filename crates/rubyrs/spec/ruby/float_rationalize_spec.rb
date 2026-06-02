# Adapted from ruby/spec core/float/rationalize_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-polished:
# - `.should ==` → `assert_eq`; `should raise_error` → `assert_raises`.
# - Both `bare rationalize` (half-ULP default eps) and `rationalize(eps)`
#   (Stern-Brocot in ±|eps|) are implemented in C.4.3 via BigInt
#   arithmetic. No-bignum tier falls back to lossless to_r for both
#   forms (Stern-Brocot needs arbitrary-precision); the test cases
#   below cover the bignum behavior.
# - `nil` eps is REJECTED with TypeError (CRuby surfaces NoMethodError
#   via `nil.abs`; rubyrs cleans up the shape).

describe "Float#rationalize" do
  it "returns exactly-representable values as their simple Rational form" do
    # These results coincide with `to_r` (already simplest); they
    # work on both bignum and no-bignum tiers.
    assert_eq(0.5.rationalize, Rational(1, 2))
    assert_eq(0.25.rationalize, Rational(1, 4))
    assert_eq(0.0.rationalize, Rational(0, 1))
  end

  bignum_it "bignum: returns the simplest Rational round-tripping to self (default half-ULP eps)" do
    # Inexact-decimal floats: bignum runs Stern-Brocot on the
    # half-ULP interval. No-bignum falls back to lossless to_r
    # (Stern-Brocot needs BigInt arithmetic).
    assert_eq(0.1.rationalize, Rational(1, 10))
    assert_eq(3.14.rationalize, Rational(157, 50))
    assert_eq(1.5.rationalize, Rational(3, 2))
    assert_eq((-0.1).rationalize, Rational(-1, 10))
  end

  bignum_it "bignum: returns the simplest fraction within ±|eps| when eps is given" do
    assert_eq(3.14.rationalize(0.01), Rational(22, 7))
    assert_eq(3.14.rationalize(0.001), Rational(135, 43))
    assert_eq((-3.14).rationalize(0.001), Rational(-135, 43))
  end

  it "returns the lossless to_r when eps == 0" do
    assert_eq(0.5.rationalize(0.0), Rational(1, 2))
  end

  it "raises FloatDomainError on NaN / ±Infinity" do
    assert_raises("FloatDomainError") { (0.0 / 0.0).rationalize }
    assert_raises("FloatDomainError") { (1.0 / 0.0).rationalize }
  end

  it "raises TypeError when eps is nil or non-Numeric" do
    # CRuby raises NoMethodError 'undefined method abs for nil';
    # rubyrs surfaces the cleaner TypeError shape.
    assert_raises("TypeError") { 0.1.rationalize(nil) }
    assert_raises("TypeError") { 0.1.rationalize(:sym) }
    assert_raises("TypeError") { 0.1.rationalize("x") }
  end

  it "raises ArgumentError if passed more than one argument" do
    assert_raises("ArgumentError") { 0.5.rationalize(1, 2) }
  end
end
