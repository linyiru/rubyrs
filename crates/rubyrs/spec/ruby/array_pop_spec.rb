
# rubyrs-spec-extract v0.4: 2 pattern(s) left for hand polish.
# Each entry names the upstream line + reason. Address each
# (comment out, inline, or wait for a later extractor version)
# before the file is consumable by the micro-runner.
#
#   - L99: `mock` — no mock library in the micro-runner; hand-translate
#   - L100: `should_receive` — mock expectations; hand-translate

describe "Array#pop" do
  it "removes and returns the last element of the array" do
    a = ["a", 1, nil, true]

    assert_eq(a.pop, true)
    assert_eq(a, ["a", 1, nil])

    assert_eq(a.pop, nil)
    assert_eq(a, ["a", 1])

    assert_eq(a.pop, 1)
    assert_eq(a, ["a"])

    assert_eq(a.pop, "a")
    assert_eq(a, [])
  end

  it "returns nil if there are no more elements" do
    assert_eq([].pop, nil)
  end

  # skipped (fixture-dependent): it "properly handles recursive arrays" do

  # skipped (fixture-dependent): it "raises a FrozenError on a frozen array" do

  # skipped (fixture-dependent): it "raises a FrozenError on an empty frozen array" do

  describe "passed a number n as an argument" do
  # skipped (fixture-dependent): it "removes and returns an array with the last n elements of the array" do

  # skipped (fixture-dependent): it "returns an array with the last n elements even if shift was invoked" do

  # skipped (fixture-dependent): it "returns a new empty array if there are no more elements" do

  # skipped (fixture-dependent): it "returns whole elements if n exceeds size of the array" do

  # skipped (fixture-dependent): it "does not return self even when it returns whole elements" do

  # skipped (fixture-dependent): it "raises an ArgumentError if n is negative" do

  # skipped (fixture-dependent): it "tries to convert n to an Integer using #to_int" do

  # skipped (fixture-dependent): it "raises a TypeError when the passed n cannot be coerced to Integer" do

  # skipped (fixture-dependent): it "raises an ArgumentError if more arguments are passed" do

  # skipped (fixture-dependent): it "does not return subclass instances with Array subclass" do

  # skipped (fixture-dependent): it "raises a FrozenError on a frozen array" do
  end
end
