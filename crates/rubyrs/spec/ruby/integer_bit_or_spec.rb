# Adapted from ruby/spec core/integer/bit_or_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-polished using the same
# conventions as integer_bit_and_spec.rb (sibling file in this
# batch): `context` → flattened, `bignum_value(N)` → `(2**64+N)`,
# `@bignum` shared state → inlined per-it, mock-based coerce/
# to_int tests dropped.

describe "Integer#|" do
  it "fixnum: returns self bitwise OR other (pure Int operands)" do
    assert_eq((1 | 0), 1)
    assert_eq((5 | 4), 5)
    assert_eq((5 | 6), 7)
    assert_eq((248 | 4096), 4344)
  end

  bignum_it "fixnum: returns self bitwise OR other (rhs is bignum)" do
    # Same saturation rationale as integer_bit_and_spec.rb.
    assert_eq((0xffff | (2**64 + 0xf0f0)), 0x1_0000_0000_0000_ffff)
  end

  it "fixnum: returns self bitwise OR other when one operand is negative" do
    assert_eq(((1 << 33) | -1), -1)
    assert_eq((-1 | (1 << 33)), -1)
    assert_eq(((-(1 << 33) - 1) | 5), -8589934593)
    assert_eq((5 | (-(1 << 33) - 1)), -8589934593)
  end

  it "fixnum: returns self bitwise OR other when both operands are negative" do
    assert_eq((-5 | -1), -1)
    assert_eq((-3 | -4), -3)
    assert_eq((-12 | -13), -9)
    assert_eq((-13 | -12), -9)
  end

  it "fixnum: returns self bitwise OR a bignum" do
    assert_eq((-1 | 2**64), -1)
  end

  it "fixnum: raises a TypeError when passed a Float" do
    assert_raises("TypeError") do
      3 | 3.4
    end
  end

  bignum_it "bignum: returns self bitwise OR other" do
    bn = 2**64 + 11
    assert_eq((bn | 2), 18446744073709551627)
    assert_eq((bn | 9), 18446744073709551627)
    assert_eq((bn | 2**64), 18446744073709551627)
  end

  bignum_it "bignum: returns self bitwise OR other when one operand is negative" do
    bn = 2**64 + 11
    assert_eq((bn | -0x40000000000000000), -55340232221128654837)
    assert_eq((bn | -bn), -1)
    assert_eq((bn | -0x8000000000000000), -9223372036854775797)
  end

  bignum_it "bignum: returns self bitwise OR other when both operands are negative" do
    bn = 2**64 + 11
    assert_eq((-bn | -0x4000000000000005), -1)
    assert_eq((-bn | -bn), -18446744073709551627)
    assert_eq((-bn | -0x4000000000000000), -11)
  end

  bignum_it "bignum: raises a TypeError when passed a Float" do
    bn = 2**64 + 11
    assert_raises("TypeError") do
      bn | 9.9
    end
  end
end
