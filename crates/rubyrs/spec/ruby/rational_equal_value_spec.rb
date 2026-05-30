# Adapted from ruby/spec core/rational/equal_value_spec.rb
# (Rational#==) at upstream commit 448cb340 (2026-05).
# Hand-polished:
# - `.should ==` → `assert_eq`; negative cases use `assert_neq`
#   (the micro-runner has no `refute` helper).

describe "Rational#== when given a Rational" do
  it "returns true for equal canonical values" do
    assert_eq(Rational(1, 2), Rational(1, 2))
    assert_eq(Rational(2, 4), Rational(1, 2))   # canonical-form aware
    assert_eq(Rational(-3, 4), Rational(3, -4)) # both normalize to (-3, 4)
  end

  it "returns false for unequal values" do
    assert_neq(Rational(1, 2), Rational(1, 3))
    assert_neq(Rational(2, 3), Rational(3, 2))
  end
end

describe "Rational#== when given an Integer" do
  it "returns true when the Rational equals the Integer" do
    assert_eq(Rational(5, 1), 5)
    assert_eq(Rational(10, 2), 5)
    assert_eq(Rational(0, 7), 0)
    assert_eq(Rational(-3, 1), -3)
  end

  it "returns false when the Rational has a fractional part" do
    assert_neq(Rational(1, 2), 1)
    assert_neq(Rational(7, 3), 2)
  end
end

describe "Rational#== when given a Float" do
  it "returns true when the Float equals the Rational value" do
    assert_eq(Rational(1, 2), 0.5)
    assert_eq(Rational(1, 4), 0.25)
    assert_eq(Rational(0, 1), 0.0)
  end

  it "returns false when the Float doesn't equal the Rational value" do
    assert_neq(Rational(1, 3), 0.5)
    assert_neq(Rational(1, 2), 0.75)
  end
end

describe "Rational#== when given non-Numeric" do
  it "returns false for non-numeric arguments" do
    assert_neq(Rational(1, 2), "1/2")
    assert_neq(Rational(1, 2), nil)
    assert_neq(Rational(1, 2), :sym)
  end
end
