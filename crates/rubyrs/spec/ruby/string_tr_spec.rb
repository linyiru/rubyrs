# Adapted from ruby/spec core/string/tr_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — full set-syntax
# coverage: literal chars, range shorthand (`a-y` → a..y),
# `^`-negation, empty-to_str delete, longer-source last-char
# stretch.

describe "String#tr" do
  it "translates each char from the source set to the corresponding char in the dest set" do
    assert_eq("hello".tr("el", "ip"), "hippo")
  end

  it "uses the last char of the dest set when the source set is longer" do
    assert_eq("hello".tr("aeiou", "*"), "h*ll*")
  end

  it "returns a new string (non-destructive)" do
    s = "hello"
    assert_eq(s.tr("l", "r"), "herro")
    assert_eq(s, "hello")
  end

  it "returns self unchanged when no source chars match" do
    assert_eq("hello".tr("xyz", "abc"), "hello")
  end

  it "translates characters in the range" do
    assert_eq("hello".tr("a-y", "b-z"), "ifmmp")
    assert_eq("hello".tr("a-y", "A-Y"), "HELLO")
  end

  it "treats a leading ^ in from_str as negation" do
    assert_eq("hello".tr("^aeiou", "*"), "*e**o")
  end

  it "deletes negated characters when to_str is empty" do
    assert_eq("hello".tr("^aeiou", ""), "eo")
  end

  it "deletes set characters when to_str is empty" do
    assert_eq("hello".tr("aeiou", ""), "hll")
  end

  it "treats `^` literally in to_str (not as negation)" do
    # `^` is only a negation prefix in from_str; in to_str it
    # maps positionally like any other char.
    assert_eq("a".tr("a", "^b"), "^")
  end

  it "uses the LAST occurrence's index for duplicate chars in from_str" do
    # CRuby builds the translation table by overwriting per-char
    # entries, so duplicates in `from` resolve to the last
    # paired `to` char.
    assert_eq("a".tr("aa", "12"), "2")
    assert_eq("a".tr("aaa", "123"), "3")
  end

  it "raises ArgumentError on a reversed range" do
    assert_raises("ArgumentError") { "abc".tr("c-a", "x") }
  end

  describe "String#tr!" do
    it "modifies self in place and returns self" do
      s = "hello"
      assert(s.tr!("l", "r").equal?(s))
      assert_eq(s, "herro")
    end

    it "returns nil if no changes were made" do
      assert_eq("hello".tr!("xyz", "abc"), nil)
    end

    it "raises ArgumentError on a reversed range" do
      assert_raises("ArgumentError") { "abc".tr!("c-a", "x") }
    end
  end
end
