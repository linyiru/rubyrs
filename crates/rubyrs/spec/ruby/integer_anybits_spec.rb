# Adapted from ruby/spec core/integer/anybits_spec.rb at upstream
# commit 448cb340 (2026-05). Same hand-polish conventions as
# integer_allbits_spec.rb (sibling) — see that file for details.

describe "Integer#anybits?" do
  it "returns true if and only if any of the bits of the argument are set in the receiver" do
    assert_eq(42.anybits?(42), true)
    assert_eq(0b1010_1010.anybits?(0b1000_0010), true)
    assert_eq(0b1010_1010.anybits?(0b1000_0001), true)
    assert_eq(0b1000_0010.anybits?(0b0010_1100), false)
  end

  bignum_it "bignum: returns true if and only if any of the bits of the argument are set in the receiver" do
    bn = 2**64
    diff = (2 * bn) & (~bn)
    assert_eq((0b1010_1010 | diff).anybits?(0b1000_0010 | bn), true)
    assert_eq((0b1010_1010 | diff).anybits?(0b0010_1100 | bn), true)
    assert_eq((0b1000_0010 | diff).anybits?(0b0010_1100 | bn), false)
  end

  it "handles negative values using two's complement notation" do
    assert_eq((~42).anybits?(42), false)
    assert_eq((-42).anybits?(-42), true)
    assert_eq((~0b100).anybits?(~0b1), true)
  end

  bignum_it "bignum: handles negative values using two's complement notation" do
    bn = 2**64
    assert_eq((~(0b100 | bn)).anybits?(~(0b1 | bn)), true)
  end

  it "raises a TypeError when given a non-Integer" do
    assert_raises("TypeError") { 13.anybits?("10") }
    assert_raises("TypeError") { 13.anybits?(:symbol) }
    assert_raises("TypeError") { 13.anybits?(3.5) }
  end

  # skipped (mock): the "coerces the rhs using to_int" and
  # mock-based TypeError tests — same rationale as
  # integer_allbits_spec.rb.
end
