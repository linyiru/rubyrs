# Adapted from ruby/spec core/{integer,float}/{integer,numerator,
# denominator,finite,infinite}_spec.rb at 2026-05. The per-class
# helpers in preamble/numeric.rb: integer? / numerator / denominator
# / finite? / infinite? / fdiv for Integer, Float, and Rational.

describe "Integer numeric helpers" do
  it "integer? is true" do
    assert_eq(5.integer?, true)
    assert_eq((-5).integer?, true)
  end

  it "numerator / denominator are self over 1" do
    assert_eq(5.numerator, 5)
    assert_eq(5.denominator, 1)
    assert_eq((-7).numerator, -7)
    assert_eq((-7).denominator, 1)
  end

  it "finite? / infinite?" do
    assert_eq(5.finite?, true)
    assert_eq(5.infinite?, nil)
  end
end

describe "Float numeric helpers" do
  it "integer? is false" do
    assert_eq(1.5.integer?, false)
  end

  it "numerator / denominator use the exact IEEE fraction" do
    assert_eq(0.5.numerator, 1)
    assert_eq(0.5.denominator, 2)
  end

  it "fdiv divides as a Float" do
    assert_eq(7.5.fdiv(2), 3.75)
  end
end

describe "Rational numeric helpers" do
  it "integer? is false" do
    assert_eq(Rational(3, 4).integer?, false)
  end

  it "finite? / infinite?" do
    assert_eq(Rational(3, 4).finite?, true)
    assert_eq(Rational(3, 4).infinite?, nil)
  end

  it "fdiv divides as a Float" do
    assert_eq(Rational(3, 4).fdiv(2), 0.375)
  end

  it "rationalize returns self with no argument" do
    assert_eq(Rational(3, 4).rationalize, Rational(3, 4))
  end

  it "rationalize finds the simplest fraction within eps" do
    assert_eq(Rational(1, 3).rationalize(Rational(1, 10)), Rational(1, 3))
  end
end
