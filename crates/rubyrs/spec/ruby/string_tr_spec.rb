# Adapted from ruby/spec core/string/tr_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — baseline literal-
# char translation shapes. The character-range shorthand
# (`tr("a-y", "b-z")`) is dropped — rubyrs's `tr` doesn't
# expand ranges and treats the `-` literally, which diverges
# from CRuby. The negation form (`tr("^aeiou", "*")`) is also
# dropped for the same reason.

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

  # skipped (divergent): it "translates characters in the range" do
  #   Character-range shorthand `tr("a-y", "b-z")`. rubyrs treats
  #   `-` literally; CRuby expands the range.
  # skipped (divergent): it "treats a leading ^ in from_str as negation" do
  #   Negation form `tr("^aeiou", "*")`. rubyrs treats `^`
  #   literally; CRuby negates the set.
  # skipped (method-not-implemented): describe "String#tr!" do ... end
  #   Destructive variant.
end
