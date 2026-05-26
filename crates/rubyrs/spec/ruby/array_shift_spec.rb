# Adapted from ruby/spec core/array/shift_spec.rb at
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

  # skipped (fixture): it "properly handles recursive arrays" do

  # skipped (fixture): it "raises a FrozenError on a frozen array" do
  # skipped (fixture): it "raises a FrozenError on an empty frozen array" do

  describe "passed a number n as an argument" do
    # skipped (method-not-implemented): it "removes and returns an array with the first n element of the array" do

    # skipped (method-not-implemented): it "does not corrupt the array when shift without arguments is followed by shift with an argument" do

    # skipped (method-not-implemented): it "returns a new empty array if there are no more elements" do

    # skipped (method-not-implemented): it "returns whole elements if n exceeds size of the array" do

    # skipped (method-not-implemented): it "does not return self even when it returns whole elements" do

    # skipped (method-not-implemented): it "raises an ArgumentError if n is negative" do

    # skipped (mock): it "tries to convert n to an Integer using #to_int" do

    # skipped (method-not-implemented): it "raises a TypeError when the passed n cannot be coerced to Integer" do

    # skipped (method-not-implemented): it "raises an ArgumentError if more arguments are passed" do

    # skipped (fixture): it "does not return subclass instances with Array subclass" do
  end
end
