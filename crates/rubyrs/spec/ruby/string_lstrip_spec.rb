# Adapted from ruby/spec core/string/lstrip_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — upstream
# includes the bang variant `String#lstrip!` (not in subset)
# and a shared body.

describe "String#lstrip" do
  it "returns a copy of self with leading whitespace removed" do
    assert_eq("  hello  ".lstrip, "hello  ")
    assert_eq("  hello world  ".lstrip, "hello world  ")
    assert_eq("\n\r\t\n\v\r hello world  ".lstrip, "hello world  ")
    assert_eq("hello".lstrip, "hello")
    assert_eq(" こにちわ".lstrip, "こにちわ")
  end

  it "works with lazy substrings" do
    assert_eq("  hello  "[1...-1].lstrip, "hello ")
    assert_eq("  hello world  "[1...-1].lstrip, "hello world ")
    assert_eq("\n\r\t\n\v\r hello world  "[1...-1].lstrip, "hello world ")
    assert_eq("   こにちわ "[1...-1].lstrip, "こにちわ")
  end

  it "strips leading \\0" do
    assert_eq("\x00hello".lstrip, "hello")
    assert_eq("\000 \000hello\000 \000".lstrip, "hello\000 \000")
  end

  describe "String#lstrip!" do
    it "modifies self in place and returns self" do
      s = "  hello"
      assert(s.lstrip!.equal?(s))
      assert_eq(s, "hello")
    end

    it "returns nil if no modifications were made" do
      assert_eq("hello".lstrip!, nil)
    end
  end
end
