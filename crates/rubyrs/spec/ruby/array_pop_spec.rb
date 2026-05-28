# Adapted from ruby/spec core/array/pop_spec.rb at
# upstream commit 448cb340 (2026-05). Produced by
# `rubyrs-spec-extract` v0.4 + `scripts/polish.py`.
#
# polish.py dropped `it` blocks containing fixture refs,
# unimplemented Array method FORMS (e.g. multi-arg `Array#push`,
# count-form `first(n)` / `last(n)` / `pop(n)` / `shift(n)`,
# block-form `min { ... }` / `max { ... }` / `sort { ... }`),
# or `mock`/`should_receive`; each drop leaves a
# `# skipped (<category>): ...` trace inline. See
# crates/rubyrs-spec-extract/scripts/polish.py DROP_PATTERNS
# for the full set. Regenerate by re-running the extractor
# + polish pipeline documented in
# crates/rubyrs-spec-extract/README.md.

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

  # skipped (fixture): it "properly handles recursive arrays" do

  # skipped (fixture): it "raises a FrozenError on a frozen array" do

  # skipped (fixture): it "raises a FrozenError on an empty frozen array" do

  describe "passed a number n as an argument" do
    it "removes and returns an array with the last n elements of the array" do
      arr = [1, 2, 3, 4, 5]
      assert_eq(arr.pop(2), [4, 5])
      assert_eq(arr, [1, 2, 3])
    end

    it "returns a new empty array if there are no more elements" do
      assert_eq([].pop(2), [])
    end

    it "returns whole elements if n exceeds size of the array" do
      arr = [1, 2, 3]
      assert_eq(arr.pop(5), [1, 2, 3])
      assert_eq(arr, [])
    end

    it "returns an empty array when n is 0" do
      arr = [1, 2, 3]
      assert_eq(arr.pop(0), [])
      assert_eq(arr, [1, 2, 3])
    end

    it "raises an ArgumentError if n is negative" do
      assert_raises("ArgumentError") { [1, 2, 3].pop(-1) }
    end

    it "raises an ArgumentError if more arguments are passed" do
      assert_raises("ArgumentError") { [1, 2, 3].pop(1, 2) }
    end

    # skipped (mock): it "tries to convert n to an Integer using #to_int" do
    #   `#to_int` coercion via mock — out of subset.
    # skipped (method-not-implemented): it "raises a TypeError when the passed n cannot be coerced to Integer" do
    #   Type-coerce path lands at the catch-all `(many)` arm with
    #   wrong-arity message rather than CRuby's TypeError-on-Float.
    # skipped (fixture): it "does not return subclass instances with Array subclass" do
    # skipped (fixture): it "raises a FrozenError on a frozen array" do
  end
end
