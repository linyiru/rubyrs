# Adapted from ruby/spec core/rational/{floor,ceil,round,truncate}_spec.rb at 2026-05.
# Covers the rounding family added in vm/dispatch.rs via
# rational_round_op: no-arg forms return an Integer, ndigits > 0
# returns a Rational, ndigits < 0 rounds to a power of ten, and
# round's `half:` keyword selects the tie-break.

describe "Rational#floor" do
  it "returns the largest Integer <= self with no argument" do
    assert_eq(Rational(7, 2).floor, 3)
    assert_eq(Rational(-7, 2).floor, -4)
    assert_eq(Rational(3, 1).floor, 3)
  end

  it "returns a Rational for positive precision" do
    assert_eq(Rational(1, 3).floor(2), Rational(33, 100))
  end

  it "returns an Integer for negative precision" do
    assert_eq(Rational(1234, 1).floor(-2), 1200)
  end
end

describe "Rational#ceil" do
  it "returns the smallest Integer >= self with no argument" do
    assert_eq(Rational(7, 2).ceil, 4)
    assert_eq(Rational(-7, 2).ceil, -3)
  end

  it "returns a Rational for positive precision" do
    assert_eq(Rational(1, 3).ceil(2), Rational(17, 50))
  end

  it "returns an Integer for negative precision" do
    assert_eq(Rational(1234, 1).ceil(-2), 1300)
  end
end

describe "Rational#truncate" do
  it "truncates toward zero with no argument" do
    assert_eq(Rational(7, 2).truncate, 3)
    assert_eq(Rational(-7, 2).truncate, -3)
  end

  it "truncates toward zero for positive precision" do
    assert_eq(Rational(-1, 3).truncate(2), Rational(-33, 100))
  end
end

describe "Rational#round" do
  it "rounds half away from zero by default" do
    assert_eq(Rational(7, 2).round, 4)
    assert_eq(Rational(5, 2).round, 3)
    assert_eq(Rational(-5, 2).round, -3)
    assert_eq(Rational(1, 3).round, 0)
  end

  it "returns a Rational for positive precision" do
    assert_eq(Rational(1, 3).round(2), Rational(33, 100))
    assert_eq(Rational(10, 3).round(5), Rational(333333, 100000))
  end

  it "returns an Integer for negative precision" do
    assert_eq(Rational(99, 1).round(-2), 100)
    assert_eq(Rational(149, 1).round(-2), 100)
    assert_eq(Rational(150, 1).round(-2), 200)
  end

  it "honors the half: keyword on exact ties" do
    assert_eq(Rational(1, 2).round(half: :up), 1)
    assert_eq(Rational(1, 2).round(half: :down), 0)
    assert_eq(Rational(1, 2).round(half: :even), 0)
    assert_eq(Rational(3, 2).round(half: :even), 2)
    assert_eq(Rational(5, 2).round(half: :even), 2)
  end
end
