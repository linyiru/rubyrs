# Adapted from ruby/spec core/rational/plus_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-polished:
# - `.should ==` → `assert_eq`.
# - `bignum_value` cases gated as `bignum_it` so they only run
#   on the profile where `try_rational_binop` carries
#   arbitrary-precision arithmetic (Phase C.4.1 widening).
#   Without bignum, i64 overflow still raises RangeError.

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

describe "Rational#+ with BigInt-magnitude operands" do
  bignum_it "bignum: stays Rational when the intermediate sum exceeds i64" do
    # Operands fit i64 individually; in `try_rational_binop`'s
    # checked-i64 add, the cross-multiplied terms `10**18 * 7`
    # and `10**18 * 3` both fit, but their sum `10**19` does not
    # (i64::MAX ≈ 9.22e18). Pre-C.4.1 raised RangeError; widened
    # storage now carries the BigInt through.
    assert_eq(Rational(10**18, 3) + Rational(10**18, 7),
              Rational(10 * 10**18, 21))
  end

  bignum_it "bignum: accepts a BigInt receiver" do
    bn = 2**64
    assert_eq(Rational(bn, 1) + Rational(1, 1), Rational(bn + 1, 1))
  end
end
