# Adapted from ruby/spec core/integer/gcd_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-polished:
# - `.should ==` → `assert_eq`; `should.is_a?(Integer)` → `assert(x.is_a?(Integer))`.
# - `bignum` literal `9999**99` → kept as-is; bignum_it gating.

describe "Integer#gcd" do
  it "returns self if equal to the argument" do
    assert_eq(1.gcd(1), 1)
    assert_eq(398.gcd(398), 398)
  end

  it "returns an Integer" do
    assert(36.gcd(6).is_a?(Integer))
    assert(4.gcd(20981).is_a?(Integer))
  end

  it "returns the greatest common divisor of self and argument" do
    assert_eq(10.gcd(5), 5)
    assert_eq(200.gcd(20), 20)
  end

  it "returns a positive integer even if self is negative" do
    assert_eq((-12).gcd(6), 6)
    assert_eq((-100).gcd(100), 100)
  end

  it "returns a positive integer even if the argument is negative" do
    assert_eq(12.gcd(-6), 6)
    assert_eq(100.gcd(-100), 100)
  end

  it "returns a positive integer even if both self and argument are negative" do
    assert_eq((-12).gcd(-6), 6)
    assert_eq((-100).gcd(-100), 100)
  end

  bignum_it "accepts a Bignum argument" do
    bn = 9999**99
    assert(bn.is_a?(Integer))
    assert_eq(99.gcd(bn), 99)
  end

  bignum_it "works if self is a Bignum" do
    bn = 9999**99
    assert(bn.is_a?(Integer))
    assert_eq(bn.gcd(99), 99)
  end

  it "raises an ArgumentError if not given an argument" do
    assert_raises("ArgumentError") { 12.gcd }
  end

  it "raises an ArgumentError if given more than one argument" do
    assert_raises("ArgumentError") { 12.gcd(30, 20) }
  end

  it "raises a TypeError unless the argument is an Integer" do
    assert_raises("TypeError") { 39.gcd(3.8) }
    assert_raises("TypeError") { 45872.gcd([]) }
  end
end
