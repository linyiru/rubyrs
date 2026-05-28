# Adapted from ruby/spec core/string/squeeze_spec.rb at
# upstream commit 448cb340 (2026-05). Hand-translated —
# baseline shapes for no-arg and single-arg literal-set
# squeeze plus the range / ^-negation forms (which now go
# through the shared `parse_tr_set` per the refactor that
# replaced squeeze's literal-only parser). The multi-arg
# intersection form is dropped — rubyrs's arm only accepts
# 0 or 1 selector arg.

describe "String#squeeze" do
  it "collapses every consecutive run with no argument" do
    assert_eq("aaabbbccc".squeeze, "abc")
    assert_eq("Mississippi".squeeze, "Misisipi")
  end

  it "squeezes only chars in the literal set" do
    assert_eq("aabbcc".squeeze("a"), "abbcc")
    assert_eq("aabbcc".squeeze("ab"), "abcc")
  end

  it "leaves runs unchanged when no set member matches" do
    assert_eq("aabbcc".squeeze("xyz"), "aabbcc")
  end

  it "expands range shorthand `a-z`" do
    # All of e, l, o fall in a..z — the doubled `l` collapses.
    assert_eq("hello".squeeze("a-z"), "helo")
  end

  it "treats a leading `^` as negation" do
    # `^l` = "every char except l" — the doubled `l` is OUT
    # of the negated set, so it's NOT squeezed.
    assert_eq("hello".squeeze("^l"), "hello")
    # `^c` over "aaabbb" — all chars are not-c, all squeezed.
    assert_eq("aaabbb".squeeze("^c"), "ab")
  end

  it "raises ArgumentError on a reversed range" do
    assert_raises("ArgumentError") { "hello".squeeze("c-a") }
  end

  # skipped (method-not-implemented): it "squeezes the intersection of two or more args" do
  #   Multi-arg intersection form (`"hello".squeeze("a-z", "^l")`).
  #   rubyrs's squeeze arm only accepts 0 or 1 selector arg.
  describe "String#squeeze!" do
    it "modifies self in place and returns self" do
      s = "aabbcc"
      assert(s.squeeze!.equal?(s))
      assert_eq(s, "abc")
    end

    it "returns nil if no changes were made" do
      assert_eq("abc".squeeze!, nil)
    end

    it "honours the literal-set selector" do
      s = "aabbcc"
      s.squeeze!("a")
      assert_eq(s, "abbcc")
    end
  end
end
