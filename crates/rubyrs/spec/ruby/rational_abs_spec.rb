# Adapted from ruby/spec core/rational/{abs,uminus,abs2}_spec.rb at 2026-05.
# Hand-polished: `.should ==` → `assert_eq`. Covers the unary
# Rational arms added alongside the rounding/predicate family in
# vm/dispatch.rs (abs / magnitude / -@ / +@ / abs2).

describe "Rational#abs" do
  it "returns the absolute value as a Rational" do
    assert_eq(Rational(-3, 4).abs, Rational(3, 4))
    assert_eq(Rational(3, 4).abs, Rational(3, 4))
    assert_eq(Rational(0, 1).abs, Rational(0, 1))
  end

  it "keeps the canonical (positive-denominator) form" do
    assert_eq(Rational(3, -4).abs, Rational(3, 4))
    assert_eq(Rational(-5, 3).abs.class, Rational)
  end

  it "is aliased as #magnitude" do
    assert_eq(Rational(-5, 3).magnitude, Rational(5, 3))
  end

  bignum_it "bignum: takes the magnitude of a BigInt numerator" do
    bn = 2**70
    assert_eq(Rational(-bn, 3).abs, Rational(bn, 3))
  end
end

describe "Rational#-@" do
  it "negates the numerator" do
    assert_eq(-Rational(3, 4), Rational(-3, 4))
    assert_eq(-Rational(-3, 4), Rational(3, 4))
    assert_eq(-Rational(0, 1), Rational(0, 1))
  end
end

describe "Rational#+@" do
  it "returns self unchanged" do
    assert_eq(+Rational(3, 4), Rational(3, 4))
    assert_eq(+Rational(-3, 4), Rational(-3, 4))
  end
end

describe "Rational#abs2" do
  it "returns the square of the magnitude" do
    assert_eq(Rational(3, 4).abs2, Rational(9, 16))
    assert_eq(Rational(-2, 7).abs2, Rational(4, 49))
    assert_eq(Rational(0, 1).abs2, Rational(0, 1))
  end
end
