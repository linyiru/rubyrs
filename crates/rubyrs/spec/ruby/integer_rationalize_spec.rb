# Adapted from ruby/spec core/integer/rationalize_spec.rb at
# upstream commit 448cb340 (2026-05). Hand-polished:
# - `.should ==` → `assert_eq`; `should raise_error` → `assert_raises`.
# - `bignum_value` cases skipped (see integer_to_r_spec rationale).
# - skipped (method-not-implemented): Rational epsilon arg —
#   `Integer#rationalize(eps)` accepts any value but ignores it
#   for Integer receivers (eps only matters for Float#rationalize,
#   Phase C.4). CRuby allows eps to be any object that responds
#   to `<`; we accept and ignore.

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

  # skipped (method-not-implemented): BigInt receiver. Same as
  # integer_to_r_spec — Phase C.4 widens i64 num/den to BigInt.
end
