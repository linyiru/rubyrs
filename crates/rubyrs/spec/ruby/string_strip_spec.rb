# Adapted from ruby/spec core/string/strip_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — upstream includes
# the bang variant `String#strip!` (not in subset; raises
# NoMethodError) and a `shared/strip.rb` body that exercises
# encoding-aware paths.

describe "String#strip" do
  it "returns a new string with leading and trailing whitespace removed" do
    assert_eq("   hello   ".strip, "hello")
    assert_eq("   hello world   ".strip, "hello world")
    assert_eq("\tgoodbye\r\v\n".strip, "goodbye")
  end

  it "returns a copy of self without leading and trailing NULL bytes and whitespace" do
    assert_eq(" \x00 goodbye \x00 ".strip, "goodbye")
  end

  # skipped (method-not-implemented): it "modifies self in place and returns self" do
  #   String#strip! not in subset.
  # skipped (method-not-implemented): it "returns nil if no modifications where made" do
  # skipped (method-not-implemented): it "makes a string empty if it is only whitespace" do
  # skipped (method-not-implemented): it "removes leading and trailing NULL bytes and whitespace" do
  # skipped (method-not-implemented): it "raises a FrozenError on a frozen instance that is modified" do
  # skipped (method-not-implemented): it "raises a FrozenError on a frozen instance that would not be modified" do
end
