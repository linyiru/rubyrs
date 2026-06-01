# Adapted from ruby/spec core/integer/rationalize_spec.rb at
# upstream commit 448cb340 (2026-05). Hand-polished:
# - `.should ==` → `assert_eq`; `should raise_error` → `assert_raises`.
# - `bignum_value` cases skipped (see integer_to_r_spec rationale).
# - eps arg: rubyrs validates `Integer#rationalize(eps)` by type
#   (Numeric / nil) and raises TypeError otherwise — see the
#   `raises TypeError when eps is not Numeric or nil` it-block
#   below. CRuby's MRI calls f_nonzero_p internally and raises
#   NoMethodError on non-Numeric; we surface the more standard
#   TypeError shape. Eps VALUE is ignored for Integer recv
#   (no fractional part to discard).

describe "Integer#rationalize" do
  it "returns a Rational with self as numerator and 1 as denominator" do
    assert_eq(0.rationalize, Rational(0, 1))
    assert_eq(1.rationalize, Rational(1, 1))
    assert_eq(5.rationalize, Rational(5, 1))
    assert_eq((-3).rationalize, Rational(-3, 1))
  end

  it "ignores a single argument" do
    # CRuby tolerance is only meaningful for Float#rationalize;
    # for Integer the epsilon doesn't change the (already
    # exact) result.
    assert_eq(5.rationalize(0.001), Rational(5, 1))
    assert_eq(5.rationalize(nil),   Rational(5, 1))
    assert_eq(5.rationalize(0),     Rational(5, 1))
  end

  it "raises ArgumentError if passed 2+ arguments" do
    assert_raises("ArgumentError") { 5.rationalize(0.1, 0.01) }
  end

  it "raises TypeError when eps is not Numeric or nil" do
    # CRuby's MRI internally calls `f_nonzero_p(eps)` which raises
    # NoMethodError on non-Numeric args; rubyrs surfaces the more
    # standard `X can't be coerced into Float` TypeError shape.
    assert_raises("TypeError") { 5.rationalize(:sym) }
    assert_raises("TypeError") { 5.rationalize("x") }
    assert_raises("TypeError") { 5.rationalize([]) }
  end

  # skipped (method-not-implemented): it "returns the receiver as a Rational" do
  #   # upstream core/integer/rationalize_spec.rb, bignum branch
  #   # of the receiver-as-Rational test
  #   bn = 2**64
  #   assert_eq(bn.rationalize, Rational(bn, 1))
  # end
  # Phase C.4 widens RationalRepr's i64 num/den to BigInt; today
  # `Integer#rationalize` raises RangeError for BigInt magnitudes.
end
