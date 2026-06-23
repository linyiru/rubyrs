# Adapted from ruby/spec core/float/{next_float,prev_float}_spec.rb
# at 2026-05. The adjacent representable doubles (IEEE-754 nextUp /
# nextDown), covering the native float_adjacent helper.

describe "Float#next_float / #prev_float" do
  it "steps to the adjacent representable double" do
    assert_eq(1.0.next_float, 1.0000000000000002)
    assert_eq(1.0.prev_float, 0.9999999999999999)
    assert_eq(1.0.next_float.prev_float, 1.0)
  end

  it "steps off zero to the smallest subnormal" do
    assert_eq(0.0.next_float, 5.0e-324)
    assert_eq(0.0.prev_float, -5.0e-324)
    assert_eq((-0.0).next_float, 5.0e-324)
  end

  it "saturates the infinities inward and stays put outward" do
    assert_eq(Float::INFINITY.next_float, Float::INFINITY)
    assert_eq(Float::INFINITY.prev_float, Float::MAX)
    assert_eq((-Float::INFINITY).next_float, -Float::MAX)
    assert_eq((-Float::INFINITY).prev_float, -Float::INFINITY)
  end

  it "maps NaN to NaN" do
    assert_eq((0.0 / 0.0).next_float.nan?, true)
    assert_eq((0.0 / 0.0).prev_float.nan?, true)
  end
end
