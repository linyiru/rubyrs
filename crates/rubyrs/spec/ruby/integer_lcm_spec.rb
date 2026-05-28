# Adapted from ruby/spec core/integer/lcm_spec.rb at upstream
# commit 448cb340 (2026-05). Sibling to integer_gcd_spec.rb.

describe "Integer#lcm" do
  it "returns self if equal to the argument" do
    assert_eq(1.lcm(1), 1)
    assert_eq(398.lcm(398), 398)
  end

  it "returns an Integer" do
    assert(36.lcm(6).is_a?(Integer))
    assert(4.lcm(20981).is_a?(Integer))
  end

  it "returns the least common multiple of self and argument" do
    assert_eq(200.lcm(2001), 400200)
    assert_eq(99.lcm(90), 990)
  end

  it "returns a positive integer even if self is negative" do
    assert_eq((-12).lcm(6), 12)
    assert_eq((-100).lcm(100), 100)
  end

  it "returns a positive integer even if the argument is negative" do
    assert_eq(12.lcm(-6), 12)
    assert_eq(100.lcm(-100), 100)
  end

  it "returns a positive integer even if both self and argument are negative" do
    assert_eq((-12).lcm(-6), 12)
    assert_eq((-100).lcm(-100), 100)
  end

  bignum_it "accepts a Bignum argument" do
    bn = 9999**99
    assert(bn.is_a?(Integer))
    assert_eq(99.lcm(bn), bn)
  end

  bignum_it "works if self is a Bignum" do
    bn = 9999**99
    assert(bn.is_a?(Integer))
    assert_eq(bn.lcm(99), bn)
  end

  it "raises an ArgumentError if not given an argument" do
    assert_raises("ArgumentError") { 12.lcm }
  end

  it "raises an ArgumentError if given more than one argument" do
    assert_raises("ArgumentError") { 12.lcm(30, 20) }
  end

  it "raises a TypeError unless the argument is an Integer" do
    assert_raises("TypeError") { 39.lcm(3.8) }
    assert_raises("TypeError") { 45872.lcm([]) }
  end
end
