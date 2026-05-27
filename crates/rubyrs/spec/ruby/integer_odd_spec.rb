# Adapted from ruby/spec core/integer/odd_spec.rb at
# upstream commit 448cb340 (2026-05). Hand-translated — see
# integer_even_spec.rb's header for context-block / fixture
# rationale. The fixnum context's `bignum_value(...)` lines
# are dropped (one block in the file); the four bignum-context
# `it` blocks are lifted using direct `**` expressions.

describe "Integer#odd?" do
  it "returns true when self is an odd number" do
    assert_eq((-2).odd?, false)
    assert_eq((-1).odd?, true)

    assert_eq(0.odd?, false)
    assert_eq(1.odd?, true)
    assert_eq(2.odd?, false)
  end

  it "returns true if self is odd and positive" do
    assert_eq((987279**19).odd?, true)
  end

  it "returns true if self is odd and negative" do
    assert_eq((-9873389**97).odd?, true)
  end

  it "returns false if self is even and positive" do
    assert_eq((10000000**10).odd?, false)
  end

  it "returns false if self is even and negative" do
    assert_eq((-1000000**100).odd?, false)
  end

  # skipped (fixture): it "<bignum_value lines from fixnum context>"
  #   Upstream's fixnum context mixes `bignum_value(0)` /
  #   `bignum_value(1)` cases with the literal cases above; the
  #   literal subset is covered by the lifted `it` blocks.
end
