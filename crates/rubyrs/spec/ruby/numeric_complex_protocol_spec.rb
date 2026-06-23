# Adapted from ruby/spec core/numeric/{real,imaginary,conjugate,
# rectangular,polar,arg,abs2,real}_spec.rb at 2026-05. The shared
# complex-decomposition protocol CRuby installs on Numeric: every
# plain real number (Integer / Float / Rational) reports itself as
# the real part with a zero imaginary part. Covers preamble/numeric.rb.

describe "Numeric complex-decomposition protocol" do
  it "real / imaginary / imag" do
    assert_eq(5.real, 5)
    assert_eq(5.imaginary, 0)
    assert_eq(1.5.imag, 0)
    assert_eq(Rational(3, 4).real, Rational(3, 4))
    assert_eq(Rational(3, 4).imaginary, 0)
  end

  it "conjugate / conj return self" do
    assert_eq(5.conjugate, 5)
    assert_eq(1.5.conj, 1.5)
    assert_eq(Rational(3, 4).conjugate, Rational(3, 4))
  end

  it "real? is true for every plain numeric" do
    assert_eq(5.real?, true)
    assert_eq(1.5.real?, true)
    assert_eq(Rational(3, 4).real?, true)
  end

  it "rectangular / rect pair the value with a zero imaginary part" do
    assert_eq(5.rectangular, [5, 0])
    assert_eq(1.5.rect, [1.5, 0])
    assert_eq(Rational(3, 4).rectangular, [Rational(3, 4), 0])
  end

  it "arg / angle / phase: 0 when non-negative, PI when negative" do
    assert_eq(5.arg, 0)
    assert_eq(0.arg, 0)
    assert_eq((-5).arg, Math::PI)
    assert_eq((-1.5).angle, Math::PI)
    assert_eq(Rational(-1, 2).phase, Math::PI)
  end

  it "polar is [abs, arg]" do
    assert_eq(5.polar, [5, 0])
    assert_eq((-5).polar, [5, Math::PI])
  end

  it "abs2 is the square of the magnitude" do
    assert_eq(5.abs2, 25)
    assert_eq((-5).abs2, 25)
    assert_eq(1.5.abs2, 2.25)
    assert_eq(Rational(3, 4).abs2, Rational(9, 16))
  end

  it "magnitude aliases abs" do
    assert_eq((-5).magnitude, 5)
    assert_eq((-1.5).magnitude, 1.5)
    assert_eq(Rational(-3, 4).magnitude, Rational(3, 4))
  end
end
