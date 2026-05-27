# Adapted from ruby/spec core/integer/allbits_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-polished:
# - `.should ==` → `assert_eq`.
# - `bignum_value` → `(2**64)`.
# - `bignum_value` arms gated as `bignum_it` so they only run
#   under the bignum profile (no-bignum saturates the `2**64`
#   literal to i64::MAX, breaking the two's-complement masking).
# - skipped (mock): the "coerces the rhs using to_int" test
#   (`obj.should_receive(:to_int).and_return(0b10)`) requires
#   mspec's mock library. CRuby's allbits? coerces via #to_int,
#   but rubyrs raises TypeError on non-Integer rhs — tracked as
#   a follow-up (Numeric#coerce / to_int protocol).
# - skipped (mock): the embedded mock inside the TypeError test
#   (`obj.should_receive(:coerce)`). The plain non-Numeric cases
#   ("10" / :symbol) are kept as a non-mock subset.

describe "Integer#allbits?" do
  it "returns true if and only if all the bits of the argument are set in the receiver" do
    assert_eq(42.allbits?(42), true)
    assert_eq(0b1010_1010.allbits?(0b1000_0010), true)
    assert_eq(0b1010_1010.allbits?(0b1000_0001), false)
    assert_eq(0b1000_0010.allbits?(0b1010_1010), false)
  end

  bignum_it "bignum: returns true if and only if all the bits of the argument are set in the receiver" do
    bn = 2**64
    assert_eq((0b1010_1010 | bn).allbits?(0b1000_0010 | bn), true)
    assert_eq((0b1010_1010 | bn).allbits?(0b1000_0001 | bn), false)
    assert_eq((0b1000_0010 | bn).allbits?(0b1010_1010 | bn), false)
  end

  it "handles negative values using two's complement notation" do
    assert_eq((~0b1).allbits?(42), true)
    assert_eq((-42).allbits?(-42), true)
    assert_eq((~0b1010_1010).allbits?(~0b1110_1011), true)
    assert_eq((~0b1010_1010).allbits?(~0b1000_0010), false)
  end

  bignum_it "bignum: handles negative values using two's complement notation" do
    bn = 2**64
    assert_eq((~(0b1010_1010 | bn)).allbits?(~(0b1110_1011 | bn)), true)
    assert_eq((~(0b1010_1010 | bn)).allbits?(~(0b1000_0010 | bn)), false)
  end

  it "raises a TypeError when given a non-Integer" do
    assert_raises("TypeError") { 13.allbits?("10") }
    assert_raises("TypeError") { 13.allbits?(:symbol) }
    assert_raises("TypeError") { 13.allbits?(3.5) }
  end

  # skipped (mock): "coerces the rhs using to_int" — needs
  # mspec's `mock(...).should_receive(:to_int)`. The to_int
  # coerce protocol isn't wired through rubyrs's bit-op path
  # (which would also re-cover the mock-only "should_receive
  # :coerce" leg of the TypeError test).
end
