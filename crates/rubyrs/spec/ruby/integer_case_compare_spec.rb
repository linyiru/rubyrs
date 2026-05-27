# Adapted from ruby/spec core/integer/case_compare_spec.rb +
# core/integer/shared/equal.rb at upstream commit 448cb340
# (2026-05). Upstream is `it_behaves_like :integer_equal, :===`
# against the shared/equal.rb block; inline the runnable cases
# for `===` here.
#
# CRuby's `Integer#===` is the alias for `Integer#==` used by
# `case`/`when`, so the value-equality semantics are identical to
# `Integer#==` (see integer_equal_value_spec.rb). Pin the
# selector independently because `===` dispatches through
# ruby_eq (not the BinOp == path), so a regression in one path
# wouldn't necessarily surface in the other.
#
# Hand-polish:
# - `.should ==` → `assert_eq`.
# - `bignum_value(N)` → `(2**64 + N)`; `before :each @bignum`
#   inlined per `bignum_it`.
# - skipped (mock): the "calls 'other == self' if the given
#   argument is not an Integer" and "returns the result of
#   'other == self' as a boolean" tests — both require mspec's
#   mock library.
# `ruby_eq`'s BigInt×Float arms (heap.rs) now route through the
# same `bigint_equals_float_lossless` helper PR #230 added for
# the BinOp `==` path, so `(2**64) === (2**64).to_f` returns
# true (2^64 is exact in f64) and the divergent skip is gone.

describe "Integer#===" do
  it "fixnum: returns true if self has the same Integer value as other" do
    assert_eq(1 === 1, true)
    assert_eq(9 === 5, false)
    assert_eq(0 === 0, true)
    assert_eq((-7) === -7, true)
  end

  it "fixnum: returns true when comparing with a Float of the same numeric value" do
    assert_eq(9 === 9.0, true)
    assert_eq(9 === 9.01, false)
    assert_eq(0 === 0.0, true)
  end

  it "fixnum: returns false when comparing with a non-Numeric argument" do
    assert_eq(1 === "*", false)
    assert_eq(1 === nil, false)
    assert_eq(1 === :one, false)
    assert_eq(1 === [1], false)
  end

  bignum_it "fixnum: returns false when comparing with a Bignum-range Integer" do
    assert_eq(10 === 2**64, false)
    assert_eq(0 === 2**64, false)
  end

  bignum_it "bignum: returns true if self has the same value as the given argument" do
    bn = 2**64
    assert_eq(bn === bn, true)
    # Two structurally-distinct allocations with the same value.
    assert_eq(bn === (2**32 * 2**32), true)
  end

  bignum_it "bignum: returns false when comparing with a different Integer value" do
    bn = 2**64
    assert_eq(bn === bn + 1, false)
    assert_eq((bn + 1) === bn, false)
    assert_eq(bn === (2**64 + 10), false)
  end

  bignum_it "bignum: returns false when comparing with a Fixnum-range Integer" do
    bn = 2**64
    assert_eq(bn === 9, false)
    assert_eq(bn === 0, false)
    assert_eq(bn === -1, false)
  end

  bignum_it "bignum: returns false when comparing with a Float with a different numeric value" do
    bn = 2**64
    assert_eq(bn === 9.01, false)
  end

  bignum_it "bignum: returns true when comparing with a Float of the same numeric value" do
    bn = 2**64
    # 2^64 is exact in f64 (power of 2 below f64's exponent ceiling),
    # so the lossless ruby_eq path treats the BigInt and the Float
    # as equal — matching the BinOp `==` semantics PR #230 pinned.
    assert_eq(bn === bn.to_f, true)
  end
  #
  # bignum_it "bignum: returns true when comparing with a Float of the same numeric value" do
  #   bn = 2**64
  #   assert_eq(bn === bn.to_f, true)
  # end

  # skipped (mock): "calls 'other == self' if the given argument
  # is not an Integer" + "returns the result of 'other == self'
  # as a boolean" — both need mspec mocks.
end
