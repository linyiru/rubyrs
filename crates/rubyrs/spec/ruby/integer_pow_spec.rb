# Adapted from ruby/spec core/integer/pow_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-polished:
# - `context "two arguments are passed"` flattened (the micro-
#   runner doesn't define `context`); the lifted `it` names
#   include "pow(exp, mod)" to keep the contextual hint.
# - skipped (fixture): `it_behaves_like :integer_exponent, :pow`
#   — shared-example registry the micro-runner doesn't model.
#   The 1-arg `Integer#pow(exp)` form is already covered by
#   tests/embed/numeric.rs::pow_* (Phase B.5a embed tests).
# - skipped (fixture): `Rational(12, 1)` arg case — Rational
#   isn't in the rubyrs subset (Phase C item).
# - The "works well with bignums" case is rewritten to use
#   `assert_eq` (CRuby's `.eql?` is value+type-strict, but for
#   `Integer#pow(exp, mod) -> Integer` the result is always
#   Integer so `eql?` reduces to `==` here).
# - The error-message regex `/2nd argument not allowed unless
#   all arguments are integers/` is dropped — we assert just the
#   error class. rubyrs's exact message text is pinned in
#   tests/embed/numeric.rs::pow_*; spec coverage here is for
#   "raises TypeError" not "raises with exact text".

describe "Integer#pow" do
  it "pow(exp, mod) returns modulo of self raised to the given power" do
    assert_eq(2.pow(5, 12), 8)
    assert_eq(2.pow(6, 13), 12)
    assert_eq(2.pow(7, 14), 2)
    assert_eq(2.pow(8, 15), 1)
  end

  it "pow(exp, mod) works well with bignum-sized modulus" do
    assert_eq(2.pow(61, 5843009213693951), 3697379018277258)
    assert_eq(2.pow(62, 5843009213693952), 1551748822859776)
    assert_eq(2.pow(63, 5843009213693953), 3103497645717974)
    assert_eq(2.pow(64, 5843009213693954), 363986077738838)
  end

  it "pow(exp, mod) handles sign like #divmod does (floor-mod)" do
    # Explicit parens around `(-2)` for reader clarity — Ruby
    # treats `-2` as a literal token (so `-2.pow(...)` parses
    # as `(-2).pow(...)`, NOT as `-(2.pow(...))`), matching CRuby.
    # But the unary-vs-literal distinction trips readers up,
    # and the upstream ruby/spec convention is the bare form;
    # spelling out the receiver here avoids any ambiguity.
    assert_eq(2.pow(5, 12), 8)
    assert_eq(2.pow(5, -12), -4)
    assert_eq((-2).pow(5, 12), 4)
    assert_eq((-2).pow(5, -12), -8)
  end

  it "pow(exp, mod) ensures all arguments are integers" do
    assert_raises("TypeError") do
      2.pow(5, 12.0)
    end
  end

  it "pow(exp, mod) raises TypeError for non-numeric modulus" do
    assert_raises("TypeError") do
      2.pow(5, "12")
    end
    assert_raises("TypeError") do
      2.pow(5, [])
    end
    assert_raises("TypeError") do
      2.pow(5, nil)
    end
  end

  it "pow(exp, mod) raises ZeroDivisionError when modulus is 0" do
    assert_raises("ZeroDivisionError") do
      2.pow(5, 0)
    end
  end

  it "pow(exp, mod) raises RangeError when exp is negative" do
    assert_raises("RangeError") do
      2.pow(-5, 1)
    end
  end
end
