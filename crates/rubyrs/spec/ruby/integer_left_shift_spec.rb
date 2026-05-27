# Adapted from ruby/spec core/integer/left_shift_spec.rb at
# upstream commit 448cb340 (2026-05). Hand-polished:
# - `context` → flattened, `fixnum_max`/`fixnum_min` → i64
#   bounds (sibling rationale to integer_bit_length_spec),
#   `bignum_value` → `2**64`, `@bignum` shared state inlined.
# - skipped (mock): `#to_int` coerce / mock-based tests.
# - skipped (divergent): the upstream RangeError-on-huge-shift
#   block at L195-218 — rubyrs raises ResourceExhausted (the
#   DoS-cap path in try_bigint_bit_shift, see PR #159 docs)
#   rather than CRuby's "shift width too big" RangeError.
#   Documented divergence; covered by
#   tests/embed/numeric.rs::bigint_shift_left_traps_dos_via_max_value_bytes
#   and friends. The divergence is intentional: tight cap means
#   the cap message tells the embedder "you can raise this if
#   you really want huge shifts" rather than CRuby's hard-
#   coded RangeError.

describe "Integer#<<" do
  it "fixnum: returns n shifted left m bits when n > 0, m > 0" do
    assert_eq((1 << 1), 2)
  end

  it "fixnum: returns n shifted left m bits when n < 0, m > 0" do
    assert_eq((-1 << 1), -2)
    assert_eq((-7 << 1), -14)
    assert_eq((-42 << 2), -168)
  end

  it "fixnum: returns n shifted right m bits when n > 0, m < 0" do
    assert_eq((2 << -1), 1)
  end

  it "fixnum: returns n shifted right m bits when n < 0, m < 0" do
    assert_eq((-2 << -1), -1)
  end

  it "fixnum: returns 0 when n == 0" do
    assert_eq((0 << 1), 0)
  end

  it "fixnum: returns n when n > 0, m == 0" do
    assert_eq((1 << 0), 1)
  end

  it "fixnum: returns n when n < 0, m == 0" do
    assert_eq((-1 << 0), -1)
  end

  it "fixnum: returns 0 when n > 0, m < 0 and n < 2**-m" do
    assert_eq((3 << -2), 0)
    assert_eq((7 << -3), 0)
    assert_eq((127 << -7), 0)
    assert_eq((7 << -32), 0)
    assert_eq((7 << -64), 0)
  end

  it "fixnum: returns -1 when n < 0, m < 0 and n > -(2**-m)" do
    assert_eq((-3 << -2), -1)
    assert_eq((-7 << -3), -1)
    assert_eq((-127 << -7), -1)
    assert_eq((-7 << -32), -1)
    assert_eq((-7 << -64), -1)
  end

  it "fixnum: raises a TypeError when passed nil" do
    assert_raises("TypeError") do
      3 << nil
    end
  end

  it "fixnum: raises a TypeError when passed a String" do
    assert_raises("TypeError") do
      3 << "4"
    end
  end

  bignum_it "bignum: returns 0 when m < 0 and m is a Bignum (fixnum recv)" do
    assert_eq((3 << -(2**64)), 0)
  end

  bignum_it "bignum: returns a Bignum == fixnum_max * 2 when fixnum_max << 1 and n > 0" do
    # fixnum_max = i64::MAX. Pre-PR #159 (B.3) this wrapped to
    # i64::MIN; the round-trip overflow detection in
    # try_int_shl_lossless promotes correctly.
    result = 9223372036854775807 << 1
    assert(result.instance_of?(Integer))
    assert_eq(result, 9223372036854775807 * 2)
  end

  bignum_it "bignum: returns a Bignum == fixnum_min * 2 when fixnum_min << 1 and n < 0" do
    result = (-9223372036854775808) << 1
    assert(result.instance_of?(Integer))
    assert_eq(result, (-9223372036854775808) * 2)
  end

  bignum_it "bignum: returns n shifted left m bits when n > 0, m > 0" do
    bn = 2**67  # = bignum_value * 8 upstream
    assert_eq((bn << 4), 2361183241434822606848)
  end

  bignum_it "bignum: returns n shifted left m bits when n < 0, m > 0" do
    bn = 2**67
    assert_eq((-bn << 9), -75557863725914323419136)
  end

  bignum_it "bignum: returns n shifted right m bits when n > 0, m < 0" do
    bn = 2**67
    assert_eq((bn << -1), 73786976294838206464)
  end

  bignum_it "bignum: returns n shifted right m bits when n < 0, m < 0" do
    bn = 2**67
    assert_eq((-bn << -2), -36893488147419103232)
  end

  bignum_it "bignum: returns n when m == 0" do
    bn = 2**67
    assert_eq((bn << 0), bn)
    assert_eq((-bn << 0), -bn)
  end

  bignum_it "bignum: returns 0 when m < 0 and m == p where 2**p > n >= 2**(p-1)" do
    bn = 2**67
    assert_eq((bn << -68), 0)
  end

  bignum_it "bignum: returns Fixnum == fixnum_max when (fixnum_max * 2) << -1 and n > 0 (demote-on-fit)" do
    result = (9223372036854775807 * 2) << -1
    assert(result.instance_of?(Integer))
    assert_eq(result, 9223372036854775807)
  end

  bignum_it "bignum: returns Fixnum == fixnum_min when (fixnum_min * 2) << -1 and n < 0 (demote-on-fit)" do
    result = ((-9223372036854775808) * 2) << -1
    assert(result.instance_of?(Integer))
    assert_eq(result, -9223372036854775808)
  end

  bignum_it "bignum: returns -1 when m < 0 (Bignum) and n < 0" do
    assert_eq((-1 << -(2**64)), -1)
    assert_eq((-1 << -(2**40)), -1)
    assert_eq((-(2**64) << -(2**64)), -1)
    assert_eq((-(2**64) << -(2**40)), -1)
  end

  bignum_it "bignum: returns 0 when m < 0 (Bignum) and n >= 0" do
    assert_eq((0 << -(2**64)), 0)
    assert_eq((1 << -(2**64)), 0)
    assert_eq(((2**64) << -(2**64)), 0)
    assert_eq((0 << -(2**40)), 0)
    assert_eq((1 << -(2**40)), 0)
    assert_eq(((2**64) << -(2**40)), 0)
  end

  it "fixnum: returns 0 for m == 0 with a large (long-sized) shift count" do
    # Upstream splits "m > 0 and n == 0" into a 'long' (fits i64)
    # and a 'bignum' case. The long-sized case is meaningful under
    # no-bignum too — `2**40` fits i64 and exercises the
    # `(*b as u32).min(63)` clamp at numeric.rs:445. Keep it on
    # both profiles so a regression in the long-count clamp is
    # caught on no-bignum CI; gate only the bignum-count case.
    assert_eq((0 << (2**40)), 0)
  end

  bignum_it "bignum: returns 0 for m == 0 with a bignum-sized shift count" do
    assert_eq((0 << (2**64)), 0)
  end
end
