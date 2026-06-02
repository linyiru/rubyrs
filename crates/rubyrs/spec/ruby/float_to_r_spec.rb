# Adapted from ruby/spec core/float/to_r_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-polished:
# - `.should ==` → `assert_eq`; `should raise_error` → `assert_raises`.
# - `Float#to_r` is lossless (CRuby behavior): the result is the
#   exact `f = sign * mantissa * 2^exp` Rational, even when this
#   produces a giant numerator/denominator for floats like 0.1.

describe "Float#to_r" do
  it "returns a Rational equal to self for exactly-representable values" do
    assert_eq(0.0.to_r, Rational(0, 1))
    assert_eq((-0.0).to_r, Rational(0, 1))
    assert_eq(0.5.to_r, Rational(1, 2))
    assert_eq(0.25.to_r, Rational(1, 4))
    assert_eq((-0.5).to_r, Rational(-1, 2))
    assert_eq(3.0.to_r, Rational(3, 1))
  end

  it "returns the exact IEEE-754 Rational for inexact decimals" do
    # 0.1 is not exactly representable in IEEE-754; its lossless
    # Rational has a 53-bit numerator over a power-of-2 denominator.
    assert_eq(0.1.to_r, Rational(3602879701896397, 36028797018963968))
  end

  it "raises FloatDomainError on NaN / ±Infinity" do
    assert_raises("FloatDomainError") { (0.0 / 0.0).to_r }
    assert_raises("FloatDomainError") { (1.0 / 0.0).to_r }
    assert_raises("FloatDomainError") { (-1.0 / 0.0).to_r }
  end

  it "raises ArgumentError if passed any arguments" do
    assert_raises("ArgumentError") { 0.5.to_r(1) }
  end
end
