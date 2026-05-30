# Adapted from ruby/spec core/rational/denominator_spec.rb at
# upstream commit 448cb340 (2026-05). Hand-polished:
# - `.should ==` → `assert_eq`.

describe "Rational#denominator" do
  it "returns the denominator" do
    assert_eq(Rational(3, 4).denominator, 4)
    assert_eq(Rational(-5, 7).denominator, 7)
    assert_eq(Rational(5).denominator, 1)
  end

  it "returns a denominator > 0 even when constructed with negative den" do
    assert_eq(Rational(3, -4).denominator, 4)
    assert_eq(Rational(-3, -4).denominator, 4)
  end

  it "returns the reduced-form denominator after gcd normalization" do
    assert_eq(Rational(6, 4).denominator, 2)
    assert_eq(Rational(10, 5).denominator, 1)
  end
end
