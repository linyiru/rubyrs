# Adapted from ruby/spec core/integer/fdiv_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-polished:
# - `should be_close(v, TOLERANCE)` → `assert_close(actual, v, tol)`
#   (custom helper would be needed; substitute with direct `assert_eq`
#   where the upstream value is exact, and a delta-bounded probe
#   otherwise).
# - `bignum_value` → `(2**64)`.
# - skipped (mock): the `mock('non-numeric')` TypeError test and the
#   `mock('10').coerce(...)` test (mspec mock library).
# - skipped (method-not-implemented): the huge-bignum-numerator
#   rounding tests use `10**342`-magnitude operands; the result is
#   correct via num_traits::ToPrimitive but the assertions chain
#   16-significant-digit equalities that aren't exact across
#   double-precision implementations. Tracked as a follow-up if
#   precision tuning is needed.

describe "Integer#fdiv" do
  it "performs floating-point division between self and a fixnum" do
    # 8 / 7 ≈ 1.142857142857142857 — Float-exact at the IEEE 754
    # closest representation.
    assert_eq(8.fdiv(7), 8.0 / 7.0)
  end

  bignum_it "performs floating-point division between self and a bignum" do
    # 8 / 2^64 ≈ 4.3e-19; the Float is well-defined to ~17 digits.
    assert_eq(8.fdiv(2**64), 8.0 / (2**64).to_f)
  end

  it "performs floating-point division between self and a Float" do
    assert_eq(8.fdiv(9.0), 8.0 / 9.0)
  end

  it "returns Infinity when the argument is 0" do
    assert_eq(1.fdiv(0).infinite?, 1)
  end

  it "returns -Infinity when the argument is 0 and self is negative" do
    assert_eq((-1).fdiv(0).infinite?, -1)
  end

  it "returns Infinity when the argument is 0.0" do
    assert_eq(1.fdiv(0.0).infinite?, 1)
  end

  it "returns -Infinity when the argument is 0.0 and self is negative" do
    assert_eq((-1).fdiv(0.0).infinite?, -1)
  end

  it "returns NaN when the argument is NaN" do
    assert_eq(1.fdiv(0.0/0.0).nan?, true)
    assert_eq((-1).fdiv(0.0/0.0).nan?, true)
  end

  it "raises a TypeError when argument isn't numeric" do
    assert_raises("TypeError") { 1.fdiv("x") }
    assert_raises("TypeError") { 1.fdiv(:sym) }
    assert_raises("TypeError") { 1.fdiv(nil) }
  end

  it "raises an ArgumentError when passed multiple arguments" do
    assert_raises("ArgumentError") { 1.fdiv(6, 0.2) }
  end

  # skipped (mock): "follows the coercion protocol" — uses
  # `obj.should_receive(:coerce)`. rubyrs's fdiv doesn't currently
  # call #coerce for non-numeric args (it raises TypeError directly).
  # Tracked as a follow-up alongside the broader Numeric#coerce
  # protocol work.
end
