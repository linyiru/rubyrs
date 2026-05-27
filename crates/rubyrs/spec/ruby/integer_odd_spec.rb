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

  # Same bignum-only gating as integer_even_spec.rb's
  # `**`-literal cases — the values overflow i64 and saturate
  # via `i64::saturating_pow` to `i64::MAX = 9223372036854775807`
  # (odd) under no-bignum. Negative literals like
  # `-9873389**97` parse as `-(9873389**97)` because `**` binds
  # tighter than unary `-` (see crates/rubyrs/tests/diff/power.rb:82-84),
  # so the positive saturation is then negated to
  # `-i64::MAX = -9223372036854775807` (also odd). All four
  # saturated values end up odd, so under no-bignum the
  # "odd" assertions trivially pass and the "even" assertions
  # trivially fail — neither outcome exercises the BigInt
  # `odd?` path the spec was written for.
  bignum_it "returns true if self is odd and positive" do
    assert_eq((987279**19).odd?, true)
  end

  bignum_it "returns true if self is odd and negative" do
    assert_eq((-9873389**97).odd?, true)
  end

  bignum_it "returns false if self is even and positive" do
    assert_eq((10000000**10).odd?, false)
  end

  bignum_it "returns false if self is even and negative" do
    assert_eq((-1000000**100).odd?, false)
  end

  # skipped (fixture): it "<bignum_value lines from fixnum context>"
  #   Upstream's fixnum context mixes `bignum_value(0)` /
  #   `bignum_value(1)` cases with the literal cases above; the
  #   literal subset is covered by the lifted `it` blocks.
end
