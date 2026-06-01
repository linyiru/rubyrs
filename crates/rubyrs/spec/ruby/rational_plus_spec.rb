# Adapted from ruby/spec core/rational/plus_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-polished:
# - `.should ==` → `assert_eq`.
# - omitted (Phase C.4): cases that would overflow i64 in
#   checked arithmetic — `try_rational_binop` raises RangeError
#   where CRuby would promote num/den to BigInt. Not included
#   as skipped-trace blocks; lift together with C.4 widening.

describe "Rational#+ when given a Rational" do
  it "returns the sum of self and the other Rational" do
    assert_eq(Rational(1, 2) + Rational(1, 3), Rational(5, 6))
    assert_eq(Rational(1, 3) + Rational(1, 3), Rational(2, 3))
    assert_eq(Rational(-1, 2) + Rational(1, 2), Rational(0, 1))
  end

  it "returns a reduced-form Rational" do
    # 1/2 + 1/2 = 1/1
    assert_eq(Rational(1, 2) + Rational(1, 2), Rational(1, 1))
    # 1/6 + 1/6 = 1/3 (not 2/6)
    assert_eq(Rational(1, 6) + Rational(1, 6), Rational(1, 3))
  end
end

describe "Rational#+ when given an Integer" do
  it "returns a Rational sum (Integer is treated as n/1)" do
    assert_eq(Rational(1, 2) + 1, Rational(3, 2))
    assert_eq(Rational(1, 2) + 0, Rational(1, 2))
    assert_eq(Rational(3, 4) + (-1), Rational(-1, 4))
  end
end

describe "Rational#+ when given a Float" do
  it "returns a Float (Float dominates the numeric tower)" do
    assert_eq(Rational(1, 2) + 0.5, 1.0)
    assert_eq(Rational(1, 4) + 0.25, 0.5)
  end
end

describe "Rational#+ when given a non-Numeric" do
  it "raises TypeError" do
    assert_raises("TypeError") { Rational(1, 2) + "x" }
    assert_raises("TypeError") { Rational(1, 2) + nil }
    assert_raises("TypeError") { Rational(1, 2) + :sym }
  end
end
