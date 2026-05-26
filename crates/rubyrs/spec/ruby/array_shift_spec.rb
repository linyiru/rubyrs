
# rubyrs-spec-extract v0.4: 2 pattern(s) left for hand polish.
# Each entry names the upstream line + reason. Address each
# (comment out, inline, or wait for a later extractor version)
# before the file is consumable by the micro-runner.
#
#   - L100: `mock` — no mock library in the micro-runner; hand-translate
#   - L101: `should_receive` — mock expectations; hand-translate

describe "Array#shift" do
  it "removes and returns the first element" do
    a = [5, 1, 1, 5, 4]
    assert_eq(a.shift, 5)
    assert_eq(a, [1, 1, 5, 4])
    assert_eq(a.shift, 1)
    assert_eq(a, [1, 5, 4])
    assert_eq(a.shift, 1)
    assert_eq(a, [5, 4])
    assert_eq(a.shift, 5)
    assert_eq(a, [4])
    assert_eq(a.shift, 4)
    assert_eq(a, [])
  end

  it "returns nil when the array is empty" do
    assert_eq([].shift, nil)
  end

  # skipped (fixture-dependent): it "properly handles recursive arrays" do

  # skipped (fixture-dependent): it "raises a FrozenError on a frozen array" do
  # skipped (fixture-dependent): it "raises a FrozenError on an empty frozen array" do

  describe "passed a number n as an argument" do
  # skipped (fixture-dependent): it "removes and returns an array with the first n element of the array" do

  # skipped (fixture-dependent): it "does not corrupt the array when shift without arguments is followed by shift with an argument" do

  # skipped (fixture-dependent): it "returns a new empty array if there are no more elements" do

  # skipped (fixture-dependent): it "returns whole elements if n exceeds size of the array" do

  # skipped (fixture-dependent): it "does not return self even when it returns whole elements" do

  # skipped (fixture-dependent): it "raises an ArgumentError if n is negative" do

  # skipped (fixture-dependent): it "tries to convert n to an Integer using #to_int" do

  # skipped (fixture-dependent): it "raises a TypeError when the passed n cannot be coerced to Integer" do

  # skipped (fixture-dependent): it "raises an ArgumentError if more arguments are passed" do

  # skipped (fixture-dependent): it "does not return subclass instances with Array subclass" do
  end
end
