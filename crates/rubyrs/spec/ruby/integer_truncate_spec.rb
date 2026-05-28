# Adapted from ruby/spec core/integer/truncate_spec.rb +
# shared/to_i.rb + shared/integer_rounding.rb at upstream commit
# 448cb340. Same hand-polish conventions as siblings.

describe "Integer#truncate" do
  it "fixnum: returns self for to_i shape" do
    assert_eq(10.truncate, 10)
    assert_eq((-15).truncate, -15)
  end

  bignum_it "bignum: returns self" do
    bn = 2**64
    assert_eq(bn.truncate, bn)
    assert_eq((-bn).truncate, -bn)
  end

  it "returns self if not passed a precision" do
    [2, -4].each { |v| assert_eq(v.truncate, v) }
  end

  it "returns self if passed a precision of zero" do
    [2, -4].each { |v| assert_eq(v.truncate(0), v) }
  end

  it "returns itself if passed a positive precision" do
    [2, -4].each { |v| assert_eq(v.truncate(42), v) }
  end

  it "negative precision: returns an integer with at least precision.abs trailing zeros" do
    assert_eq(1832.truncate(-1), 1830)
    assert_eq(1832.truncate(-2), 1800)
    assert_eq(1832.truncate(-3), 1000)
    assert_eq((-1832).truncate(-1), -1830)
    assert_eq((-1832).truncate(-2), -1800)
    assert_eq((-1832).truncate(-3), -1000)
  end
end
