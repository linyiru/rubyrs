# Adapted from ruby/spec core/integer/nobits_spec.rb at upstream
# commit 448cb340 (2026-05). Same hand-polish conventions as
# integer_allbits_spec.rb (sibling) — see that file for details.

describe "Integer#nobits?" do
  it "returns true if and only if no bits of the argument are set in the receiver" do
    assert_eq(42.nobits?(42), false)
    assert_eq(0b1010_1010.nobits?(0b1000_0010), false)
    assert_eq(0b1010_1010.nobits?(0b1000_0001), false)
    assert_eq(0b0100_0101.nobits?(0b1010_1010), true)
  end

  bignum_it "bignum: returns true if and only if no bits of the argument are set in the receiver" do
    bn = 2**64
    diff = (2 * bn) & (~bn)
    assert_eq((0b1010_1010 | diff).nobits?(0b1000_0010 | bn), false)
    assert_eq((0b1010_1010 | diff).nobits?(0b1000_0001 | bn), false)
    assert_eq((0b0100_0101 | diff).nobits?(0b1010_1010 | bn), true)
  end

  it "handles negative values using two's complement notation" do
    assert_eq((~0b1101).nobits?(0b1101), true)
    assert_eq((-42).nobits?(-42), false)
    assert_eq((~0b1101).nobits?(~0b10), false)
  end

  bignum_it "bignum: handles negative values using two's complement notation" do
    bn = 2**64
    assert_eq((~(0b1101 | bn)).nobits?(~(0b10 | bn)), false)
  end

  it "raises a TypeError when given a non-Integer" do
    assert_raises("TypeError") { 13.nobits?("10") }
    assert_raises("TypeError") { 13.nobits?(:symbol) }
    assert_raises("TypeError") { 13.nobits?(3.5) }
  end

  # skipped (mock): the "coerces the rhs using to_int" and
  # mock-based TypeError tests — same rationale as
  # integer_allbits_spec.rb.
end
