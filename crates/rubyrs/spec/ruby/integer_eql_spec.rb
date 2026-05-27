# Adapted from ruby/spec core/integer/eql_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-polished:
# - upstream only has a `bignum` context; fixnum cases are
#   implicit (covered by sibling integer_equal_value_spec for
#   `==`, but `eql?` deserves its own pin since it intentionally
#   refuses cross-class equality). Fixnum cases added here as
#   non-`bignum_it` so they run on both profiles.
# - `bignum_value` → `(2**64)`; `bignum_value(N)` → `(2**64 + N)`.
# - skipped (method-not-implemented): the Rational case
#   (`bignum.eql?(Rational(bignum))`). Rational is Phase C work.

describe "Integer#eql?" do
  it "fixnum: returns true for the same Integer value" do
    assert_eq(1.eql?(1), true)
    assert_eq(0.eql?(0), true)
    assert_eq((-7).eql?(-7), true)
    # `-0` literal is the same Integer as `0` in Ruby — pin the
    # negative-zero parsing/canonicalisation here rather than in
    # the "different value" example.
    assert_eq(0.eql?(-0), true)
  end

  it "fixnum: returns false for a different Integer value" do
    assert_eq(1.eql?(2), false)
    assert_eq(9.eql?(-9), false)
  end

  it "fixnum: returns false for a Float with the same numeric value" do
    # eql? refuses cross-class equality even when `==` would
    # accept; `1 == 1.0` is true but `1.eql?(1.0)` is false.
    assert_eq(1.eql?(1.0), false)
    assert_eq(9.eql?(9.0), false)
  end

  it "fixnum: returns false for a non-Numeric argument" do
    assert_eq(1.eql?("1"), false)
    assert_eq(1.eql?(nil), false)
    assert_eq(1.eql?(:one), false)
    assert_eq(1.eql?([1]), false)
  end

  bignum_it "fixnum: returns false for a Bignum-range Integer" do
    # Companion to the upstream bignum context's symmetric case
    # ("returns false for a Fixnum-range Integer"). Pre-fix this
    # could go wrong if eql? cross-variant routing demoted before
    # comparing — pin both directions.
    assert_eq(42.eql?(2**64), false)
    assert_eq(0.eql?(2**64), false)
    assert_eq((-1).eql?(-(2**64)), false)
  end

  bignum_it "bignum: returns true for the same value" do
    assert_eq((2**64).eql?(2**64), true)
  end

  bignum_it "bignum: returns true across equivalent allocations" do
    # Two structurally-distinct BigInt allocations with the same
    # numeric value must compare equal — eql? is value-based, not
    # identity-based. Pre-fix a per-allocation pointer compare
    # would fail this.
    a = 2**64
    b = 2**32 * 2**32
    assert_eq(a.eql?(b), true)
  end

  bignum_it "bignum: returns false for a different Integer value" do
    assert_eq((2**64).eql?(2**64 + 1), false)
    assert_eq((2**64).eql?(2**65), false)
    assert_eq((2**64).eql?(-(2**64)), false)
  end

  bignum_it "bignum: returns false for a Float with the same numeric value" do
    assert_eq((2**64).eql?((2**64).to_f), false)
  end

  bignum_it "bignum: returns false for a Fixnum-range Integer" do
    assert_eq((2**64).eql?(42), false)
    assert_eq((2**64).eql?(0), false)
  end

  # skipped (method-not-implemented): Rational endpoint
  # (`(2**64).eql?(Rational(2**64))`). Rational is Phase C work.
end
