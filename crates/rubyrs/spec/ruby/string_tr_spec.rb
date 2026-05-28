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

  # skipped (method-not-implemented): describe "String#tr!" do ... end
  #   Destructive variant.
end
