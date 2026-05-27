# Adapted from ruby/spec core/string/rstrip_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — upstream
# includes the bang variant `String#rstrip!` (not in subset)
# and a shared body.

describe "String#rstrip" do
  it "returns a copy of self with trailing whitespace removed" do
    assert_eq("  hello  ".rstrip, "  hello")
    assert_eq("  hello world  ".rstrip, "  hello world")
    assert_eq("  hello world \n\r\t\n\v\r".rstrip, "  hello world")
    assert_eq("hello".rstrip, "hello")
    assert_eq("hello\x00".rstrip, "hello")
    assert_eq("こにちわ ".rstrip, "こにちわ")
  end

  it "works with lazy substrings" do
    assert_eq("  hello  "[1...-1].rstrip, " hello")
    assert_eq("  hello world  "[1...-1].rstrip, " hello world")
    assert_eq("  hello world \n\r\t\n\v\r"[1...-1].rstrip, " hello world")
    assert_eq(" こにちわ  "[1...-1].rstrip, "こにちわ")
  end

  it "returns a copy of self with all trailing whitespace and NULL bytes removed" do
    assert_eq("\x00 \x00hello\x00 \x00".rstrip, "\x00 \x00hello")
  end

  # skipped (method-not-implemented): it "<rstrip! variants>"
  #   String#rstrip! not in subset (8 upstream blocks).
end
