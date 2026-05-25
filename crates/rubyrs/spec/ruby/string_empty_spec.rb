# Adapted from ruby/spec core/string/empty_spec.rb at 2026-05.
# Upstream uses `.should.empty?` / `.should_not.empty?`
# predicate matchers which need MSpec's matcher block; we
# convert to direct `assert_eq(s.empty?, expected)` form.
# Skipped:
#   - Subclass identity (`MyString` fixtures)

describe "String#empty?" do
  it "returns true when the string has zero length" do
    assert_eq("".empty?, true)
  end

  it "returns false for a single ASCII char" do
    assert_eq(" ".empty?, false)
    assert_eq("a".empty?, false)
  end

  it "returns false for a multi-char string" do
    assert_eq("hello".empty?, false)
  end

  it "returns false for a string of a single null byte" do
    assert_eq("\x00".empty?, false)
  end

  it "agrees with length-zero check" do
    s = ""
    assert_eq(s.empty?, s.length == 0)
  end
end
