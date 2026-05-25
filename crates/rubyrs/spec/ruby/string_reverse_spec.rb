# Adapted from ruby/spec core/string/reverse_spec.rb at 2026-05
# (subset). Skipped:
#   - String subclass identity (`MyString` from fixtures)
#   - Multi-byte / broken-encoding chars (rubyrs treats strings
#     as byte sequences; codepoint-aware reverse is a separate
#     feature)
#   - `force_encoding`

describe "String#reverse" do
  it "returns a new String with the characters of self in reverse order" do
    assert_eq("stressed".reverse, "desserts")
  end

  it "returns the same string for a single-character source" do
    assert_eq("m".reverse, "m")
  end

  it "returns the empty string when self is empty" do
    assert_eq("".reverse, "")
  end

  it "does not mutate self" do
    s = "hello"
    s.reverse
    assert_eq(s, "hello")
  end

  it "produces a fresh object (not self)" do
    s = "abc"
    r = s.reverse
    assert_eq(r.equal?(s), false)
    assert_eq(r, "cba")
  end
end
