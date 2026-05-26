
# rubyrs-spec-extract v0.4: 3 pattern(s) left for hand polish.
# Each entry names the upstream line + reason. Address each
# (comment out, inline, or wait for a later extractor version)
# before the file is consumable by the micro-runner.
#
#   - L57: `mock` — no mock library in the micro-runner; hand-translate
#   - L58: `should_receive` — mock expectations; hand-translate
#   - L66: `mock` — no mock library in the micro-runner; hand-translate

describe "Array#last" do
  it "returns the last element" do
    assert_eq([1, 1, 1, 1, 2].last, 2)
  end

  it "returns nil if self is empty" do
    assert_eq([].last, nil)
  end

  # skipped (fixture-dependent): it "returns the last count elements if given a count" do

  # skipped (fixture-dependent): it "returns an empty array when passed a count on an empty array" do

  # skipped (fixture-dependent): it "returns an empty array when count == 0" do

  # skipped (fixture-dependent): it "returns an array containing the last element when passed count == 1" do

  # skipped (fixture-dependent): it "raises an ArgumentError when count is negative" do

  # skipped (fixture-dependent): it "returns the entire array when count > length" do

  # skipped (fixture-dependent): it "returns an array which is independent to the original when passed count" do

  # skipped (fixture-dependent): it "properly handles recursive arrays" do

  # skipped (fixture-dependent): it "tries to convert the passed argument to an Integer using #to_int" do

  # skipped (fixture-dependent): it "raises a TypeError if the passed argument is not numeric" do

  # skipped (fixture-dependent): it "does not return subclass instance on Array subclasses" do

  # skipped (fixture-dependent): it "is not destructive" do
end
