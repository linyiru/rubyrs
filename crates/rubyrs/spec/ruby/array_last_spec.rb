# Adapted from ruby/spec core/array/last_spec.rb at
# upstream commit 448cb340 (2026-05). Produced by
# `rubyrs-spec-extract` v0.4 + `scripts/polish.py`.
#
# polish.py dropped `it` blocks containing fixture refs,
# unimplemented Array method FORMS, or `mock`/`should_receive`;
# each drop leaves a `# skipped (<category>): ...` trace inline.
# See crates/rubyrs-spec-extract/scripts/polish.py DROP_PATTERNS
# for the full set.
#
# Re-extracted post-PR #140 — `Array#last(n)` is now in subset
# (cap-to-length, ArgumentError on negative, block-ignored).
#
# Five upstream `it` blocks remain skipped — 4 via polish.py
# DROP_PATTERNS (one fixture-recursive, two mock-machinery,
# one fixture-subclass) plus 1 hand-added skip because the
# blanket polish rule was too coarse to handle it:
#   - .replace-based "independent" check: `Array#replace` not
#     in subset yet (would unlock when shipped).
# SPEC_STATUS.md is authoritative for the exact counts.

describe "Array#last" do
  it "returns the last element" do
    assert_eq([1, 1, 1, 1, 2].last, 2)
  end

  it "returns nil if self is empty" do
    assert_eq([].last, nil)
  end

  it "returns the last count elements if given a count" do
    assert_eq([1, 2, 3, 4, 5, 9].last(3), [4, 5, 9])
  end

  it "returns an empty array when passed a count on an empty array" do
    assert_eq([].last(0), [])
    assert_eq([].last(1), [])
  end

  it "returns an empty array when count == 0" do
    assert_eq([1, 2, 3, 4, 5].last(0), [])
  end

  it "returns an array containing the last element when passed count == 1" do
    assert_eq([1, 2, 3, 4, 5].last(1), [5])
  end

  it "raises an ArgumentError when count is negative" do
    assert_raises("ArgumentError") do
      [1, 2].last(-1)
    end
  end

  it "returns the entire array when count > length" do
    assert_eq([1, 2, 3, 4, 5, 9].last(10), [1, 2, 3, 4, 5, 9])
  end

  it "returns an array which is independent to the original when passed count" do
    ary = [1, 2, 3, 4, 5]
    ary.last(0).replace([1, 2])
    assert_eq(ary, [1, 2, 3, 4, 5])
    ary.last(1).replace([1, 2])
    assert_eq(ary, [1, 2, 3, 4, 5])
    ary.last(6).replace([1, 2])
    assert_eq(ary, [1, 2, 3, 4, 5])
  end

  # skipped (fixture): it "properly handles recursive arrays" do
  # skipped (mock): it "tries to convert the passed argument to an Integer using #to_int" do
  # skipped (mock): it "raises a TypeError if the passed argument is not numeric" do
  # skipped (fixture): it "does not return subclass instance on Array subclasses" do

  it "is not destructive" do
    a = [1, 2, 3]
    a.last
    assert_eq(a, [1, 2, 3])
    a.last(2)
    assert_eq(a, [1, 2, 3])
    a.last(3)
    assert_eq(a, [1, 2, 3])
  end
end
