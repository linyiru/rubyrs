# Adapted from ruby/spec core/array/first_spec.rb at
# upstream commit 448cb340 (2026-05). Produced by
# `rubyrs-spec-extract` v0.4 + `scripts/polish.py`.
#
# polish.py dropped `it` blocks containing fixture refs,
# unimplemented Array method FORMS (multi-arg `Array#push`,
# block-form `min { ... }`, count-form `first(n)`), or
# `mock`/`should_receive`; each drop leaves a
# `# skipped (<category>): ...` trace inline. Regenerate by
# re-running the extractor + polish pipeline documented in
# crates/rubyrs-spec-extract/README.md.
describe "Array#first" do
  it "returns the first element" do
    assert_eq(%w{a b c}.first, 'a')
    assert_eq([nil].first, nil)
  end

  it "returns nil if self is empty" do
    assert_eq([].first, nil)
  end

  # skipped (method-not-implemented): it "returns the first count elements if given a count" do

  # skipped (method-not-implemented): it "returns an empty array when passed count on an empty array" do

  # skipped (method-not-implemented): it "returns an empty array when passed count == 0" do

  # skipped (method-not-implemented): it "returns an array containing the first element when passed count == 1" do

  # skipped (method-not-implemented): it "raises an ArgumentError when count is negative" do

  # skipped (method-not-implemented): it "raises a RangeError when count is a Bignum" do

  # skipped (method-not-implemented): it "returns the entire array when count > length" do

  # skipped (method-not-implemented): it "returns an array which is independent to the original when passed count" do

  # skipped (fixture): it "properly handles recursive arrays" do

  # skipped (mock): it "tries to convert the passed argument to an Integer using #to_int" do

  # skipped (mock): it "raises a TypeError if the passed argument is not numeric" do

  # skipped (fixture): it "does not return subclass instance when passed count on Array subclasses" do

  # skipped (method-not-implemented): it "is not destructive" do
end
