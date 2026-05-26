# Adapted from ruby/spec core/array/last_spec.rb at
# upstream commit 448cb340 (2026-05). Produced by
# `rubyrs-spec-extract` v0.4 + `scripts/polish.py`.
#
# polish.py dropped `it` blocks containing fixture refs,
# unimplemented Array methods, or `mock`/`should_receive`;
# each drop leaves a `# skipped (<category>): ...` trace
# inline. Regenerate by re-running the extractor + polish
# pipeline documented in crates/rubyrs-spec-extract/README.md.
describe "Array#last" do
  it "returns the last element" do
    assert_eq([1, 1, 1, 1, 2].last, 2)
  end

  it "returns nil if self is empty" do
    assert_eq([].last, nil)
  end

  # skipped (method-not-implemented): it "returns the last count elements if given a count" do

  # skipped (method-not-implemented): it "returns an empty array when passed a count on an empty array" do

  # skipped (method-not-implemented): it "returns an empty array when count == 0" do

  # skipped (method-not-implemented): it "returns an array containing the last element when passed count == 1" do

  # skipped (method-not-implemented): it "raises an ArgumentError when count is negative" do

  # skipped (method-not-implemented): it "returns the entire array when count > length" do

  # skipped (method-not-implemented): it "returns an array which is independent to the original when passed count" do

  # skipped (fixture): it "properly handles recursive arrays" do

  # skipped (mock): it "tries to convert the passed argument to an Integer using #to_int" do

  # skipped (mock): it "raises a TypeError if the passed argument is not numeric" do

  # skipped (fixture): it "does not return subclass instance on Array subclasses" do

  # skipped (method-not-implemented): it "is not destructive" do
end
