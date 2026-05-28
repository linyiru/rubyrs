# Adapted from ruby/spec core/string/strip_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — covers both
# `String#strip` and the destructive `#strip!` sibling.
# `shared/strip.rb`'s encoding-aware paths are dropped.

describe "String#strip" do
  it "returns a new string with leading and trailing whitespace removed" do
    assert_eq("   hello   ".strip, "hello")
    assert_eq("   hello world   ".strip, "hello world")
    assert_eq("\tgoodbye\r\v\n".strip, "goodbye")
  end

  it "returns a copy of self without leading and trailing NULL bytes and whitespace" do
    assert_eq(" \x00 goodbye \x00 ".strip, "goodbye")
  end

  describe "String#strip!" do
    it "modifies self in place and returns self" do
      s = "  hello  "
      assert(s.strip!.equal?(s))
      assert_eq(s, "hello")
    end

    it "returns nil if no modifications were made" do
      assert_eq("hello".strip!, nil)
    end

    it "makes a string empty if it is only whitespace" do
      s = "   "
      s.strip!
      assert_eq(s, "")
    end

    it "removes leading and trailing NULL bytes and whitespace" do
      s = " \x00 goodbye \x00 "
      s.strip!
      assert_eq(s, "goodbye")
    end

    # skipped (fixture): it "raises a FrozenError on a frozen instance that is modified" do
    # skipped (fixture): it "raises a FrozenError on a frozen instance that would not be modified" do
  end
end
