# Adapted from ruby/spec core/array/first_spec.rb at
# upstream commit 448cb340 (2026-05). Produced by
# `rubyrs-spec-extract` v0.4 + `scripts/polish.py`.
#
# polish.py dropped `it` blocks containing fixture refs,
# unimplemented Array method FORMS, or `mock`/`should_receive`;
# each drop leaves a `# skipped (<category>): ...` trace inline.
# See crates/rubyrs-spec-extract/scripts/polish.py DROP_PATTERNS
# for the full set. Regenerate by re-running the extractor +
# polish pipeline documented in
# crates/rubyrs-spec-extract/README.md.
#
# Re-extracted post-PR #140 — `Array#first(n)` is now in subset
# (cap-to-length, ArgumentError on negative, block-ignored).
# Two upstream `it` blocks remain manually skipped because the
# blanket polish rule was too coarse to handle them:
#   - bignum_value RangeError: rubyrs caps to receiver length
#     instead of raising; behavior divergence, not unimplemented.
#   - .replace-based "independent" check: `Array#replace` not
#     in subset yet (would unlock when shipped).

describe "Array#first" do
  it "returns the first element" do
    assert_eq(%w{a b c}.first, 'a')
    assert_eq([nil].first, nil)
  end

  it "returns nil if self is empty" do
    assert_eq([].first, nil)
  end

  it "returns the first count elements if given a count" do
    assert_eq([true, false, true, nil, false].first(2), [true, false])
  end

  it "returns an empty array when passed count on an empty array" do
    assert_eq([].first(0), [])
    assert_eq([].first(1), [])
    assert_eq([].first(2), [])
  end

  it "returns an empty array when passed count == 0" do
    assert_eq([1, 2, 3, 4, 5].first(0), [])
  end

  it "returns an array containing the first element when passed count == 1" do
    assert_eq([1, 2, 3, 4, 5].first(1), [1])
  end

  it "raises an ArgumentError when count is negative" do
    assert_raises("ArgumentError") do
      [1, 2].first(-1)
    end
  end

  # skipped (divergent): it "raises a RangeError when count is a Bignum" do
  #   rubyrs caps the count to receiver length (see
  #   crates/rubyrs/tests/diff/array_first_last_n.rb:34) rather
  #   than raising RangeError; behavior divergence, not
  #   unimplemented form.

  it "returns the entire array when count > length" do
    assert_eq([1, 2, 3, 4, 5, 9].first(10), [1, 2, 3, 4, 5, 9])
  end

  # skipped (method-not-implemented): it "returns an array which is independent to the original when passed count" do
  #   Uses `ary.first(0).replace([1,2])` — Array#replace not in
  #   subset yet. Unlock when `Array#replace` ships.

  # skipped (fixture): it "properly handles recursive arrays" do
  # skipped (mock): it "tries to convert the passed argument to an Integer using #to_int" do
  # skipped (mock): it "raises a TypeError if the passed argument is not numeric" do
  # skipped (fixture): it "does not return subclass instance when passed count on Array subclasses" do

  it "is not destructive" do
    a = [1, 2, 3]
    a.first
    assert_eq(a, [1, 2, 3])
    a.first(2)
    assert_eq(a, [1, 2, 3])
    a.first(3)
    assert_eq(a, [1, 2, 3])
  end
end
