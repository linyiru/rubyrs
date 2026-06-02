# Adapted from ruby/spec core/rational/exponent_spec.rb at
# upstream commit 448cb340 (2026-05). Hand-polished:
# - `.should ==` → `assert_eq`; `should raise_error` → `assert_raises`.
# - Integer-exponent path stays exact (Rational result); non-
#   integer exponent demotes to Float (CRuby parity).

describe "Rational#**" do
  it "returns Rational(1, 1) for zero exponent" do
    assert_eq((Rational(1, 2) ** 0), Rational(1, 1))
    assert_eq((Rational(3, 4) ** 0), Rational(1, 1))
    assert_eq((Rational(-1, 2) ** 0), Rational(1, 1))
  end

  it "returns the exact Rational power for non-negative integer exponent" do
    assert_eq((Rational(1, 2) ** 3), Rational(1, 8))
    assert_eq((Rational(3, 4) ** 2), Rational(9, 16))
    assert_eq((Rational(2, 1) ** 5), Rational(32, 1))
  end

  it "returns the reciprocal for negative integer exponent" do
    assert_eq((Rational(1, 2) ** -1), Rational(2, 1))
    assert_eq((Rational(1, 2) ** -2), Rational(4, 1))
    assert_eq((Rational(2, 3) ** -2), Rational(9, 4))
  end

  it "preserves sign on odd-power negative receivers" do
    assert_eq((Rational(-1, 2) ** 3), Rational(-1, 8))
    assert_eq((Rational(-1, 2) ** 2), Rational(1, 4))
  end

  it "raises ZeroDivisionError on Rational(0) ** negative" do
    assert_raises("ZeroDivisionError") { Rational(0, 1) ** -1 }
    assert_raises("ZeroDivisionError") { Rational(0, 1) ** -5 }
  end

  it "returns a Float for non-integer exponent" do
    # CRuby's Rational#** Float fallback. The Float result for
    # `(1/2r) ** 0.5` is the lossy sqrt(0.5).
    f1 = Rational(1, 2) ** 0.5
    assert_eq(f1.class.to_s, "Float")
    assert_eq(f1, 0.7071067811865476)
  end

  it "raises TypeError on non-Numeric exponent" do
    assert_raises("TypeError") { Rational(1, 2) ** "x" }
    assert_raises("TypeError") { Rational(1, 2) ** :sym }
  end
end
