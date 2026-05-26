# Adapted from ruby/spec core/array/reverse_spec.rb at
# upstream commit 448cb340 (2026-05). Produced by
# `rubyrs-spec-extract` v0.4 + `scripts/polish.py`.
#
# polish.py dropped `it` blocks containing fixture refs,
# unimplemented Array methods, or `mock`/`should_receive`;
# each drop leaves a `# skipped (<category>): ...` trace
# inline. Regenerate by re-running the extractor + polish
# pipeline documented in crates/rubyrs-spec-extract/README.md.

describe "Array#reverse" do
  it "returns a new array with the elements in reverse order" do
    assert_eq([].reverse, [])
    assert_eq([1, 3, 5, 2].reverse, [2, 5, 3, 1])
  end

  # skipped (fixture): it "properly handles recursive arrays" do

  # skipped (fixture): it "does not return subclass instance on Array subclasses" do
end

describe "Array#reverse!" do
  it "reverses the elements in place" do
    a = [6, 3, 4, 2, 1]
    assert(a.reverse!.equal?(a))
    assert_eq(a, [1, 2, 4, 3, 6])
    assert_eq([].reverse!, [])
  end

  # skipped (fixture): it "properly handles recursive arrays" do

  # skipped (fixture): it "raises a FrozenError on a frozen array" do
end
