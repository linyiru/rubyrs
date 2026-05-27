# Adapted from ruby/spec core/integer/equal_value_spec.rb +
# core/integer/shared/equal.rb at upstream commit 448cb340
# (2026-05). Hand-translated — upstream uses
# `it_behaves_like :integer_equal, :==` against shared/equal.rb;
# inline the runnable cases for `==`.
#
# Hand-polish:
# - `bignum_value` → `(2**64)`; `bignum_value(N)` → `(2**64 + N)`.
# - `before :each` with `@bignum` inlined per `bignum_it`.
# - skipped (mock): the "calls other == self" tests
#   (`obj.should_receive(:==).and_return(...)`) and the
#   "returns result of other == self as a boolean" test. The
#   micro-runner has no mock library and these explicitly
#   exercise CRuby's mock protocol. The truthy-coerce of the
#   `other.==` return value is real CRuby behavior but the
#   only way to observe it without mocks would be via a
#   user-defined class — that's enumerator/class-coverage
#   territory, out of B.6 scope.
# The "does not lose precision when comparing with a Float" case
# is active — the BigInt == Float arm now converts the Float
# losslessly (via num_traits FromPrimitive) instead of demoting
# the BigInt to f64. See bigint_equals_float_lossless in
# bignum.rs. Lt/Le/Gt/Ge still demote (tracked as a follow-up).

describe "Integer#==" do
  it "fixnum: returns true if self has the same Integer value as other" do
    assert_eq(1 == 1, true)
    assert_eq(9 == 5, false)
    assert_eq(0 == 0, true)
    assert_eq((-7) == -7, true)
  end

  it "fixnum: returns true when comparing with a Float of the same numeric value" do
    # Cross-class: `==` coerces, unlike `eql?` which refuses.
    assert_eq(9 == 9.0, true)
    assert_eq(9 == 9.01, false)
    assert_eq(0 == 0.0, true)
  end

  it "fixnum: returns false when comparing with a non-Numeric argument" do
    assert_eq(1 == "*", false)
    assert_eq(1 == nil, false)
    assert_eq(1 == :one, false)
    assert_eq(1 == [1], false)
  end

  bignum_it "fixnum: returns false when comparing with a Bignum-range Integer" do
    assert_eq(10 == 2**64, false)
    assert_eq(0 == 2**64, false)
  end

  bignum_it "bignum: returns true if self has the same value as the given argument" do
    bn = 2**64
    assert_eq(bn == bn, true)
    # Two structurally-distinct allocations with the same value
    # must compare equal — pre-fix a per-allocation identity
    # compare would fail this.
    assert_eq(bn == (2**32 * 2**32), true)
  end

  bignum_it "bignum: returns true when comparing with a Float of the same numeric value" do
    # 2**64 is exactly representable in Float (power of 2), so the
    # round-trip preserves value.
    bn = 2**64
    assert_eq(bn == bn.to_f, true)
  end

  bignum_it "bignum: returns false when comparing with a different Integer value" do
    bn = 2**64
    assert_eq(bn == bn + 1, false)
    assert_eq((bn + 1) == bn, false)
    assert_eq(bn == (2**64 + 10), false)
  end

  bignum_it "bignum: returns false when comparing with a Fixnum-range Integer" do
    bn = 2**64
    assert_eq(bn == 9, false)
    assert_eq(bn == 0, false)
    assert_eq(bn == -1, false)
  end

  bignum_it "bignum: returns false when comparing with a Float with a different numeric value" do
    bn = 2**64
    assert_eq(bn == 9.01, false)
  end

  bignum_it "bignum: does not lose precision when comparing with a Float" do
    # The Float side is the "round-down" of `2**64 + 1` (the gap
    # between consecutive f64s at 2^64 is 2); the Integer side
    # must NOT round, so `(2**64 + 1) == (2**64).to_f` is false
    # even though `(2**64) == (2**64).to_f` is true. Pin the
    # lossless `==` path added in bigint_equals_float_lossless.
    assert_eq((2**64 + 1) == (2**64).to_f, false)
    assert_eq((2**64) == (2**64).to_f, true)
  end

  # skipped (mock): "calls 'other == self' if the given argument
  # is not an Integer" / "returns the result of 'other == self'
  # as a boolean" — both require mspec's mock library.
end
