# Adapted from ruby/spec core/string/include_spec.rb at 2026-05
# (subset). Translates `.should ==` matchers to `assert_eq`.
# Skipped:
#   - `.should.include?` / `.should_not.include?` predicate
#     matchers (would need MSpec's matcher block)
#   - Subclass identity (`MyString` fixtures)
#   - `force_encoding` / encoding coercion
#   - `to_str` coercion via mocks (no mock library)
#   - TypeError on non-String input (separate spec; sub-PR)

describe "String#include? with String" do
  it "returns true if self contains other_str" do
    assert_eq("hello".include?("lo"), true)
    assert_eq("hello".include?("ol"), false)
  end

  it "returns true when both strings are empty" do
    assert_eq("".include?(""), true)
  end

  it "returns true when the RHS is empty" do
    assert_eq("a".include?(""), true)
    assert_eq("hello".include?(""), true)
  end

  it "returns false when self is empty and RHS is non-empty" do
    assert_eq("".include?("a"), false)
  end

  it "matches a substring starting at the beginning" do
    assert_eq("hello".include?("he"), true)
  end

  it "matches a substring ending at the end" do
    assert_eq("hello".include?("lo"), true)
  end

  it "matches self as a substring of self" do
    assert_eq("hello".include?("hello"), true)
  end

  it "returns false for a string longer than self" do
    assert_eq("hi".include?("hello"), false)
  end
end
