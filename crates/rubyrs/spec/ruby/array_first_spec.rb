
# rubyrs-spec-extract v0.4: 3 pattern(s) left for hand polish.
# Each entry names the upstream line + reason. Address each
# (comment out, inline, or wait for a later extractor version)
# before the file is consumable by the micro-runner.
#
#   - L63: `mock` — no mock library in the micro-runner; hand-translate
#   - L64: `should_receive` — mock expectations; hand-translate
#   - L72: `mock` — no mock library in the micro-runner; hand-translate

describe "Array#first" do
  it "returns the first element" do
    assert_eq(%w{a b c}.first, 'a')
    assert_eq([nil].first, nil)
  end

  it "returns nil if self is empty" do
    assert_eq([].first, nil)
  end

  # skipped (fixture-dependent): it "returns the first count elements if given a count" do

  # skipped (fixture-dependent): it "returns an empty array when passed count on an empty array" do

  # skipped (fixture-dependent): it "returns an empty array when passed count == 0" do

  # skipped (fixture-dependent): it "returns an array containing the first element when passed count == 1" do

  # skipped (fixture-dependent): it "raises an ArgumentError when count is negative" do

  # skipped (fixture-dependent): it "raises a RangeError when count is a Bignum" do

  # skipped (fixture-dependent): it "returns the entire array when count > length" do

  # skipped (fixture-dependent): it "returns an array which is independent to the original when passed count" do

  # skipped (fixture-dependent): it "properly handles recursive arrays" do

  # skipped (fixture-dependent): it "tries to convert the passed argument to an Integer using #to_int" do

  # skipped (fixture-dependent): it "raises a TypeError if the passed argument is not numeric" do

  # skipped (fixture-dependent): it "does not return subclass instance when passed count on Array subclasses" do

  # skipped (fixture-dependent): it "is not destructive" do
end
