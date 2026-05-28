# Adapted from ruby/spec core/string/downcase_spec.rb at
# upstream commit 448cb340 (2026-05). Hand-translated —
# baseline ASCII downcase shape plus the destructive `!`
# sibling. Unicode-option forms (`:turkic` / `:lithuanian` /
# `:fold`) are out of subset, same as the upcase spec.

describe "String#downcase" do
  it "returns a copy with ASCII letters folded to lowercase" do
    assert_eq("HELLO".downcase, "hello")
  end

  it "leaves already-lowercase chars unchanged" do
    assert_eq("hello".downcase, "hello")
    assert_eq("Hello World".downcase, "hello world")
  end

  it "returns an empty String for an empty receiver" do
    assert_eq("".downcase, "")
  end

  it "leaves non-letter chars unchanged" do
    assert_eq("Hi-5 OK?".downcase, "hi-5 ok?")
  end

  it "returns a new string (non-destructive)" do
    s = "HELLO"
    assert_eq(s.downcase, "hello")
    assert_eq(s, "HELLO")
  end

  describe "String#downcase!" do
    it "modifies self in place and returns self" do
      s = "HELLO"
      assert(s.downcase!.equal?(s))
      assert_eq(s, "hello")
    end

    it "returns nil if no changes are made" do
      assert_eq("hello".downcase!, nil)
    end
  end

  # skipped (method-not-implemented): it "respects the Unicode case-mapping options" do
  #   Option forms `downcase(:ascii)` / `:turkic` / `:lithuanian`
  #   / `:fold`. Tier-2 Encoding work (ADR 0020).
end
