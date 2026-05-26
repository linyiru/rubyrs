
# rubyrs-spec-extract v0.4: 6 pattern(s) left for hand polish.
# Each entry names the upstream line + reason. Address each
# (comment out, inline, or wait for a later extractor version)
# before the file is consumable by the micro-runner.
#
#   - L11: `mock` — no mock library in the micro-runner; hand-translate
#   - L24: `mock` — no mock library in the micro-runner; hand-translate
#   - L25: `mock` — no mock library in the micro-runner; hand-translate
#   - L26: `mock` — no mock library in the micro-runner; hand-translate
#   - L27: `should_receive` — mock expectations; hand-translate
#   - L28: `should_receive` — mock expectations; hand-translate

describe "Array#include?" do
  it "returns true if object is present, false otherwise" do
    assert_eq([1, 2, "a", "b"].include?("c"), false)
    assert_eq([1, 2, "a", "b"].include?("a"), true)
  end

  # skipped (fixture-dependent): it "determines presence by using element == obj" do

  # skipped (fixture-dependent): it "calls == on elements from left to right until success" do
end
