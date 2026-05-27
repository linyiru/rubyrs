# Adapted from ruby/spec core/integer/bit_xor_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-polished using the same
# conventions as integer_bit_and / integer_bit_or — sibling files
# in this batch.

describe "Integer#^" do
  it "fixnum: returns self bitwise EXCLUSIVE OR other (pure Int operands)" do
    assert_eq((3 ^ 5), 6)
    assert_eq((-2 ^ -255), 255)
  end

  bignum_it "fixnum: returns self bitwise EXCLUSIVE OR other (rhs is bignum)" do
    # Same saturation rationale as integer_bit_and_spec.rb.
    assert_eq((5 ^ (2**64 + 0xffff_ffff)), 0x1_0000_0000_ffff_fffa)
  end

  it "fixnum: returns self bitwise XOR other when one operand is negative" do
    assert_eq(((1 << 33) ^ -1), -8589934593)
    assert_eq((-1 ^ (1 << 33)), -8589934593)
    assert_eq(((-(1 << 33) - 1) ^ 5), -8589934598)
    assert_eq((5 ^ (-(1 << 33) - 1)), -8589934598)
  end

  it "fixnum: returns self bitwise XOR other when both operands are negative" do
    assert_eq((-5 ^ -1), 4)
    assert_eq((-3 ^ -4), 1)
    assert_eq((-12 ^ -13), 7)
    assert_eq((-13 ^ -12), 7)
  end

  it "fixnum: returns self bitwise EXCLUSIVE OR a bignum" do
    assert_eq((-1 ^ 2**64), -18446744073709551617)
  end

  it "fixnum: raises a TypeError when passed a Float" do
    assert_raises("TypeError") do
      3 ^ 3.4
    end
  end

  bignum_it "bignum: returns self bitwise EXCLUSIVE OR other" do
    bn = 2**64 + 18
    assert_eq((bn ^ 2), 18446744073709551632)
    assert_eq((bn ^ bn), 0)
    assert_eq((bn ^ 14), 18446744073709551644)
  end

  bignum_it "bignum: returns self bitwise EXCLUSIVE OR other when one operand is negative" do
    bn = 2**64 + 18
    assert_eq((bn ^ -0x40000000000000000), -55340232221128654830)
    assert_eq((bn ^ -bn), -4)
    assert_eq((bn ^ -0x8000000000000000), -27670116110564327406)
  end

  bignum_it "bignum: returns self bitwise EXCLUSIVE OR other when both operands are negative" do
    bn = 2**64 + 18
    assert_eq((-bn ^ -0x40000000000000000), 55340232221128654830)
    assert_eq((-bn ^ -bn), 0)
    assert_eq((-bn ^ -0x4000000000000000), 23058430092136939502)
  end

  bignum_it "bignum: returns self bitwise EXCLUSIVE OR other when all bits are 1 and other value is negative" do
    assert_eq((9903520314283042199192993791 ^ -1), -9903520314283042199192993792)
    assert_eq((784637716923335095479473677900958302012794430558004314111 ^ -1),
              -784637716923335095479473677900958302012794430558004314112)
  end

  bignum_it "bignum: raises a TypeError when passed a Float" do
    bn = 2**64 + 18
    assert_raises("TypeError") do
      bn ^ 9.9
    end
  end
end
