# Adapted from ruby/spec core/rational/{zero,positive,negative}_spec.rb at 2026-05.
# Also covers numeric/nonzero_spec.rb. Covers the sign-predicate Rational
# arms in vm/dispatch.rs. The sign always lives in the numerator
# because canonical form keeps the denominator positive.

describe "Rational#zero?" do
  it "is true only for a zero numerator" do
    assert_eq(Rational(0, 1).zero?, true)
    assert_eq(Rational(0, 5).zero?, true)
    assert_eq(Rational(1, 2).zero?, false)
    assert_eq(Rational(-1, 2).zero?, false)
  end
end

describe "Rational#nonzero?" do
  it "returns self when non-zero and nil when zero" do
    assert_eq(Rational(4, 2).nonzero?, Rational(2, 1))
    assert_eq(Rational(0, 1).nonzero?, nil)
  end
end

describe "Rational#positive?" do
  it "is true only for a positive value" do
    assert_eq(Rational(1, 2).positive?, true)
    assert_eq(Rational(-1, 2).positive?, false)
    assert_eq(Rational(0, 1).positive?, false)
  end
end

describe "Rational#negative?" do
  it "is true only for a negative value" do
    assert_eq(Rational(-1, 2).negative?, true)
    assert_eq(Rational(1, 2).negative?, false)
    assert_eq(Rational(0, 1).negative?, false)
  end
end
