# Adapted from ruby/spec core/integer/even_spec.rb at
# upstream commit 448cb340 (2026-05). Hand-translated (not
# extractor output) because the upstream file uses `context`
# blocks that the micro-runner's spec_helper.rb doesn't define
# — see crates/rubyrs-spec-extract/src/lib.rs:901. Each `context`
# block's runnable `it` body is lifted to a top-level `it` under
# the single `describe`; bignum cases that depended on the
# `bignum_value(N)` fixture are rewritten with direct `**`
# expressions (rubyrs's Bignum `#even?` agrees with CRuby —
# verified manually).

describe "Integer#even?" do
  it "returns true for a Fixnum when it is an even number" do
    assert_eq((-2).even?, true)
    assert_eq((-1).even?, false)

    assert_eq(0.even?, true)
    assert_eq(1.even?, false)
    assert_eq(2.even?, true)
  end

  # The four `**`-literal cases below evaluate to Bignum-sized
  # values (10^40 / 9879^976) that don't fit in i64. Under the
  # no-bignum profile they'd saturate via `i64::saturating_pow`
  # to `i64::MAX = 9223372036854775807` (odd), making
  # `.even?` return false for what was meant to be an "even
  # bignum" assertion. Gate with `bignum_it` so the cases only
  # run on the profile where they make sense; rubyrs's Bignum
  # `#even?` agrees with CRuby per `crates/rubyrs/src/vm/bignum.rs`'s
  # `even?` arm (`b & 1 == 0` on the heap-stored magnitude).
  bignum_it "returns true if self is even and positive" do
    assert_eq((10000**10).even?, true)
  end

  bignum_it "returns true if self is even and negative" do
    assert_eq((-10000**10).even?, true)
  end

  bignum_it "returns false if self is odd and positive" do
    assert_eq((9879**976).even?, false)
  end

  bignum_it "returns false if self is odd and negative" do
    assert_eq((-9879**976).even?, false)
  end

  # skipped (fixture): it "returns true for a Bignum when it is an even number" do
  #   Uses `bignum_value(N)` upstream fixture. The body
  #   exercises the same predicate the four lifted cases above
  #   already cover via direct `**` exponents.
end
