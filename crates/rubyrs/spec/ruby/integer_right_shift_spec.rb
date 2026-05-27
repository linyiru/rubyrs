# Adapted from ruby/spec core/integer/right_shift_spec.rb at
# upstream commit 448cb340 (2026-05). Same hand-polish
# conventions as integer_left_shift_spec.rb (sibling) — see
# that file's header for the documented divergences (mock-based
# coerce tests dropped, RangeError-on-huge-shift block dropped
# in favour of rubyrs's ResourceExhausted-DoS-cap divergence).

describe "Integer#>>" do
  it "fixnum: returns n shifted right m bits when n > 0, m > 0" do
    assert_eq((2 >> 1), 1)
  end

  it "fixnum: returns n shifted right m bits when n < 0, m > 0" do
    assert_eq((-2 >> 1), -1)
    assert_eq((-7 >> 1), -4)
    assert_eq((-42 >> 2), -11)
  end

  it "fixnum: returns n shifted left m bits when n > 0, m < 0" do
    assert_eq((1 >> -1), 2)
  end

  it "fixnum: returns n shifted left m bits when n < 0, m < 0" do
    assert_eq((-1 >> -1), -2)
  end

  it "fixnum: returns 0 when n == 0" do
    assert_eq((0 >> 1), 0)
  end

  it "fixnum: returns n when m == 0" do
    assert_eq((1 >> 0), 1)
    assert_eq((-1 >> 0), -1)
  end

  it "fixnum: returns 0 when n > 0, m > 0 and n < 2**m" do
    assert_eq((3 >> 2), 0)
    assert_eq((7 >> 3), 0)
    assert_eq((127 >> 7), 0)
    assert_eq((7 >> 32), 0)
    assert_eq((7 >> 64), 0)
  end

  it "fixnum: returns -1 when n < 0, m > 0 and n > -(2**m)" do
    assert_eq((-3 >> 2), -1)
    assert_eq((-7 >> 3), -1)
    assert_eq((-127 >> 7), -1)
    assert_eq((-7 >> 32), -1)
    assert_eq((-7 >> 64), -1)
  end

  it "fixnum: raises a TypeError when passed nil" do
    assert_raises("TypeError") do
      3 >> nil
    end
  end

  it "fixnum: raises a TypeError when passed a String" do
    assert_raises("TypeError") do
      3 >> "4"
    end
  end

  bignum_it "fixnum: returns a Bignum == fixnum_max * 2 when fixnum_max >> -1 and n > 0" do
    result = 9223372036854775807 >> -1
    assert(result.instance_of?(Integer))
    assert_eq(result, 9223372036854775807 * 2)
  end

  bignum_it "fixnum: returns a Bignum == fixnum_min * 2 when fixnum_min >> -1 and n < 0" do
    result = (-9223372036854775808) >> -1
    assert(result.instance_of?(Integer))
    assert_eq(result, (-9223372036854775808) * 2)
  end

  bignum_it "bignum: returns n shifted right m bits when n > 0, m > 0" do
    bn = 2**67
    assert_eq((bn >> 1), 73786976294838206464)
  end

  bignum_it "bignum: returns n shifted right m bits when n < 0, m > 0" do
    bn = 2**67
    assert_eq((-bn >> 2), -36893488147419103232)
  end

  bignum_it "bignum: respects twos complement signed shifting" do
    # Important: explicit bit patterns. The shift discards low
    # bits but in two's-complement form their absence shifts
    # the sign-extended bits left of them — verified via direct
    # CRuby cross-check.
    assert_eq((-42949672980000000000000 >> 14), -2621440001220703125)
    assert_eq((-42949672980000000000001 >> 14), -2621440001220703126)
    assert_eq((-42949672980000000000000 >> 15), -1310720000610351563)
    assert_eq((-42949672980000000000001 >> 15), -1310720000610351563)
    assert_eq((-0xfffffffffffffffff >> 32), -68719476736)
  end

  bignum_it "bignum: respects twos complement signed shifting for very large values" do
    giant = 42949672980000000000000000000000000000000000000000000000000000000000000000000000000000000000
    neg = -giant
    assert_eq((giant >> 84),
              2220446050284288846538547929770901490087453566957265138626098632812)
    assert_eq((neg >> 84),
              -2220446050284288846538547929770901490087453566957265138626098632813)
  end

  bignum_it "bignum: returns n shifted left m bits when n > 0, m < 0" do
    bn = 2**67
    assert_eq((bn >> -2), 590295810358705651712)
  end

  bignum_it "bignum: returns n shifted left m bits when n < 0, m < 0" do
    bn = 2**67
    assert_eq((-bn >> -3), -1180591620717411303424)
  end

  bignum_it "bignum: returns n when m == 0" do
    bn = 2**67
    assert_eq((bn >> 0), bn)
    assert_eq((-bn >> 0), -bn)
  end

  bignum_it "bignum: returns 0 when m > 0 and m == p where 2**p > n >= 2**(p-1)" do
    bn = 2**67
    assert_eq((bn >> 68), 0)
  end

  bignum_it "bignum: returns a Fixnum == fixnum_max when (fixnum_max * 2) >> 1 (demote-on-fit)" do
    result = (9223372036854775807 * 2) >> 1
    assert(result.instance_of?(Integer))
    assert_eq(result, 9223372036854775807)
  end

  bignum_it "bignum: returns a Fixnum == fixnum_min when (fixnum_min * 2) >> 1 (demote-on-fit)" do
    result = ((-9223372036854775808) * 2) >> 1
    assert(result.instance_of?(Integer))
    assert_eq(result, -9223372036854775808)
  end

  bignum_it "bignum: returns -1 when m > 0 (Bignum) and n < 0" do
    assert_eq((-1 >> (2**64)), -1)
    assert_eq((-1 >> (2**40)), -1)
    assert_eq((-(2**64) >> (2**64)), -1)
    assert_eq((-(2**64) >> (2**40)), -1)
  end

  bignum_it "bignum: returns 0 when m > 0 (Bignum) and n >= 0" do
    assert_eq((0 >> (2**64)), 0)
    assert_eq((1 >> (2**64)), 0)
    assert_eq(((2**64) >> (2**64)), 0)
    assert_eq((0 >> (2**40)), 0)
    assert_eq((1 >> (2**40)), 0)
    assert_eq(((2**64) >> (2**40)), 0)
  end

  it "fixnum: returns 0 for m == 0 with a large (long-sized) negative shift count" do
    # Symmetric to the left-shift split: `-(2**40)` fits i64 and
    # exercises the negative-count clamp at numeric.rs:483-484
    # (the `i64::MIN` / `(-b) as u32` boundary added in PR #159).
    # Keep on both profiles; gate only the bignum-count case.
    assert_eq((0 >> -(2**40)), 0)
  end

  bignum_it "bignum: returns 0 for m == 0 with a bignum-sized negative shift count" do
    assert_eq((0 >> -(2**64)), 0)
  end
end
