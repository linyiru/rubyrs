# Adapted from ruby/spec core/integer/bit_length_spec.rb at
# upstream commit 448cb340 (2026-05). Hand-polished:
# - `context` blocks flattened into top-level `it` / `bignum_it`
#   names (the micro-runner's spec_helper.rb doesn't define
#   `context`).
# - `fixnum_max` / `fixnum_min` upstream fixtures replaced with
#   rubyrs's i64 bounds (i64::MAX = 9223372036854775807,
#   i64::MIN = -9223372036854775808). The original assertions
#   probe whether bit-position N is 0 or 1 at the saturation
#   boundary; the substitution preserves the intent because
#   our Fixnum width is i64 (not CRuby's 62-bit Fixnum).
# - Bignum-context cases gated with `bignum_it`; under no-bignum
#   the `2**1000` literals saturate via `i64::saturating_pow` to
#   `i64::MAX` (bit_length 63), masking the spec intent.
# - The `.succ` / `.pred` lines on BigInt receivers (originally
#   commented out as subset-skipped) are uncommented after the
#   B.6 follow-up that added succ/next/pred to bigint_primitive
#   (see tests/embed/numeric.rs::
#   `bigint_succ_pred_promote_at_i64_boundary_and_demote_back`).

describe "Integer#bit_length" do
  it "returns the position of the leftmost bit of a positive number (fixnum)" do
    assert_eq(0.bit_length, 0)
    assert_eq(1.bit_length, 1)
    assert_eq(2.bit_length, 2)
    assert_eq(3.bit_length, 2)
    assert_eq(4.bit_length, 3)
    # `fixnum_max = i64::MAX = 2**63 - 1`. bit_length = 63;
    # bit 63 is 0 (the value never reaches that position), bit
    # 62 is 1 (the highest set bit of i64::MAX).
    fixnum_max = 9223372036854775807
    n = fixnum_max.bit_length
    assert_eq(fixnum_max[n], 0)
    assert_eq(fixnum_max[n - 1], 1)

    assert_eq(0xff.bit_length, 8)
    assert_eq(0x100.bit_length, 9)
    assert_eq((2**12 - 1).bit_length, 12)
    assert_eq((2**12).bit_length, 13)
    assert_eq((2**12 + 1).bit_length, 13)
  end

  it "returns the position of the leftmost 0 bit of a negative number (fixnum)" do
    assert_eq((-1).bit_length, 0)
    assert_eq((-2).bit_length, 1)
    assert_eq((-3).bit_length, 2)
    assert_eq((-4).bit_length, 2)
    assert_eq((-5).bit_length, 3)
    # `fixnum_min = i64::MIN = -2**63`. bit_length = 63; bit 63
    # is 1 (sign-bit extension), bit 62 is 0.
    fixnum_min = -9223372036854775808
    n = fixnum_min.bit_length
    assert_eq(fixnum_min[n], 1)
    assert_eq(fixnum_min[n - 1], 0)

    assert_eq((-2**12 - 1).bit_length, 13)
    assert_eq((-2**12).bit_length, 12)
    assert_eq((-2**12 + 1).bit_length, 12)
    assert_eq((-0x101).bit_length, 9)
    assert_eq((-0x100).bit_length, 8)
    assert_eq((-0xff).bit_length, 8)
    assert_eq((-2).bit_length, 1)
    assert_eq((-1).bit_length, 0)
  end

  bignum_it "returns the position of the leftmost bit of a positive number (bignum)" do
    assert_eq((2**1000-1).bit_length, 1000)
    assert_eq((2**1000).bit_length, 1001)
    assert_eq((2**1000+1).bit_length, 1001)

    assert_eq((2**10000-1).bit_length, 10000)
    assert_eq((2**10000).bit_length, 10001)
    assert_eq((2**10000+1).bit_length, 10001)

    assert_eq((1 << 100).bit_length, 101)
    assert_eq((1 << 100).succ.bit_length, 101)
    assert_eq((1 << 100).pred.bit_length, 100)
    assert_eq((1 << 10000).bit_length, 10001)
  end

  bignum_it "returns the position of the leftmost 0 bit of a negative number (bignum)" do
    assert_eq((-2**10000-1).bit_length, 10001)
    assert_eq((-2**10000).bit_length, 10000)
    assert_eq((-2**10000+1).bit_length, 10000)

    assert_eq((-2**1000-1).bit_length, 1001)
    assert_eq((-2**1000).bit_length, 1000)
    assert_eq((-2**1000+1).bit_length, 1000)

    assert_eq(((-1 << 100)-1).bit_length, 101)
    assert_eq(((-1 << 100)-1).succ.bit_length, 100)
    assert_eq(((-1 << 100)-1).pred.bit_length, 101)
    assert_eq(((-1 << 10000)-1).bit_length, 10001)
  end
end
