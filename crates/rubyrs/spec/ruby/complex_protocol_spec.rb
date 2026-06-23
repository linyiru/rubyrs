# Adapted from ruby/spec core/complex/{numerator,denominator,finite,
# infinite,zero,rationalize}_spec.rb at 2026-05. Complex methods
# beyond the arithmetic core, added to preamble/complex.rb.

describe "Complex#numerator / #denominator" do
  it "are self / 1 for integer components" do
    assert_eq(Complex(3, 4).numerator, Complex(3, 4))
    assert_eq(Complex(3, 4).denominator, 1)
  end

  it "clear the fractional parts over a common denominator" do
    c = Complex(Rational(1, 2), Rational(1, 3))
    assert_eq(c.denominator, 6)
    assert_eq(c.numerator, Complex(3, 2))
  end
end

describe "Complex#finite? / #infinite?" do
  it "fold over both components" do
    assert_eq(Complex(3, 4).finite?, true)
    assert_eq(Complex(3, 4).infinite?, nil)
    assert_eq(Complex(Float::INFINITY, 0).finite?, false)
    assert_eq(Complex(Float::INFINITY, 0).infinite?, 1)
    assert_eq(Complex(0, Float::INFINITY).infinite?, 1)
  end
end

describe "Complex#zero? / #nonzero?" do
  it "are true / nil only when both parts are zero" do
    assert_eq(Complex(0, 0).zero?, true)
    assert_eq(Complex(3, 4).zero?, false)
    assert_eq(Complex(0, 0).nonzero?, nil)
    assert_eq(Complex(3, 4).nonzero?, Complex(3, 4))
  end
end

describe "Complex#rationalize" do
  it "succeeds only for a real-valued Complex" do
    assert_eq(Complex(3, 0).rationalize, Rational(3, 1))
    assert_eq(Complex(Rational(1, 2), 0).rationalize, Rational(1, 2))
    assert_raises("RangeError") { Complex(3, 4).rationalize }
  end
end
