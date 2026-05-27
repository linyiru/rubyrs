# Adapted from ruby/spec core/integer/bit_and_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-polished:
# - `context "fixnum"` / `context "bignum"` flattened into
#   top-level `it` / `bignum_it` names (the micro-runner doesn't
#   define `context`).
# - `bignum_value(N)` substituted with `(2**64 + N)`; bare
#   `bignum_value` (no arg) = `2**64`.
# - `before :each` with shared `@bignum` inlined into each
#   bignum_it (spec_helper.rb doesn't model hooks).
# - skipped (mock): the `#coerce` / `#to_int` mock-based tests
#   are dropped — they exercise CRuby's mock library, not the
#   bit-and surface itself.
# - skipped (subset): `bignum_value + 0xffff_ffff` in the
#   "AND a positive value across the i64 boundary" line
#   substituted with a direct literal.

describe "Integer#&" do
  it "fixnum: returns self bitwise AND other (pure Int operands)" do
    assert_eq((256 & 16), 0)
    assert_eq((2010 & 5), 0)
    assert_eq((65535 & 1), 1)
  end

  bignum_it "fixnum: returns self bitwise AND other (rhs is bignum)" do
    # Upstream: `0xffff & bignum_value + 0xffff_ffff` — the
    # operator precedence means `bignum_value + 0xffff_ffff` is
    # the rhs operand. Substitute directly. Gated on bignum
    # because the (2**64 + N) literal saturates under no-bignum.
    assert_eq((0xffff & (2**64 + 0xffff_ffff)), 65535)
  end

  it "fixnum: returns self bitwise AND other when one operand is negative" do
    assert_eq(((1 << 33) & -1), (1 << 33))
    assert_eq((-1 & (1 << 33)), (1 << 33))
    assert_eq(((-(1 << 33) - 1) & 5), 5)
    assert_eq((5 & (-(1 << 33) - 1)), 5)
  end

  it "fixnum: returns self bitwise AND other when both operands are negative" do
    assert_eq((-5 & -1), -5)
    assert_eq((-3 & -4), -4)
    assert_eq((-12 & -13), -16)
    assert_eq((-13 & -12), -16)
  end

  it "fixnum: returns self bitwise AND a bignum" do
    assert_eq((-1 & 2**64), 18446744073709551616)
  end

  it "fixnum: raises a TypeError when passed a Float" do
    assert_raises("TypeError") do
      3 & 3.4
    end
  end

  bignum_it "bignum: returns self bitwise AND other" do
    bn = 2**64 + 5
    assert_eq((bn & 3), 1)
    assert_eq((bn & 52), 4)
    assert_eq((bn & (2**64 + 9921)), 18446744073709551617)

    assert_eq(((2 * 2**64) & 1), 0)
    assert_eq(((2 * 2**64) & (2 * 2**64)), 36893488147419103232)
  end

  bignum_it "bignum: returns self bitwise AND other when one operand is negative" do
    bn = 2**64 + 5
    assert_eq(((2 * 2**64) & -1), (2 * 2**64))
    assert_eq(((4 * 2**64) & -1), (4 * 2**64))
    assert_eq((bn & -0xffffffffffffff5), 18446744073709551617)
    assert_eq((bn & -bn), 1)
    assert_eq((bn & -0x8000000000000000), 18446744073709551616)
  end

  bignum_it "bignum: returns self bitwise AND other when both operands are negative" do
    bn = 2**64 + 5
    assert_eq((-bn & -0x4000000000000005), -23058430092136939525)
    assert_eq((-bn & -bn), -18446744073709551621)
    assert_eq((-bn & -0x4000000000000000), -23058430092136939520)
  end

  bignum_it "bignum: returns self bitwise AND other (multiple-of-Fixnum::MIN-bits)" do
    val = -((1 << 93) - 1)
    assert_eq((val & val), val)

    val = -((1 << 126) - 1)
    assert_eq((val & val), val)
  end

  bignum_it "bignum: raises a TypeError when passed a Float" do
    bn = 2**64 + 5
    assert_raises("TypeError") do
      bn & 3.4
    end
  end
end
