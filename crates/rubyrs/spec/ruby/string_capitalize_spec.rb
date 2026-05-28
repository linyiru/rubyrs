# Adapted from ruby/spec core/string/capitalize_spec.rb at
# upstream commit 448cb340 (2026-05). Hand-translated —
# baseline ASCII case-fold shape. The Unicode case-mapping
# options (`capitalize(:turkic)` / `:lithuanian` / `:fold` /
# `:ascii`) are dropped (Tier-2 Encoding work — ADR 0020).

describe "String#capitalize" do
  it "returns a copy with the first ASCII letter upcased and the rest lowercased" do
    assert_eq("hello".capitalize, "Hello")
    assert_eq("HELLO".capitalize, "Hello")
    assert_eq("hello world".capitalize, "Hello world")
  end

  it "returns an empty String for an empty receiver" do
    assert_eq("".capitalize, "")
  end

  it "leaves a leading non-letter unchanged and lowercases the rest" do
    assert_eq("123abc".capitalize, "123abc")
    assert_eq("123ABC".capitalize, "123abc")
  end

  it "returns a new string (non-destructive)" do
    s = "hello"
    assert_eq(s.capitalize, "Hello")
    assert_eq(s, "hello")
  end

  it "raises ArgumentError on any positional arg (option forms unsupported)" do
    # CRuby's `String#capitalize` takes an optional Unicode
    # case-mapping option symbol — we don't implement the
    # option form (ADR 0020 Tier-2 Encoding) so any arg
    # surfaces as `wrong number of arguments` rather than
    # falling through to NoMethodError.
    assert_raises("ArgumentError") { "hi".capitalize(:ascii) }
    assert_raises("ArgumentError") { "hi".capitalize(:turkic) }
  end

  # skipped (method-not-implemented): it "respects the Unicode case-mapping options" do
  #   Option forms `capitalize(:ascii)` / `:turkic` /
  #   `:lithuanian` / `:fold`. Tier-2 Encoding work (ADR 0020).
  describe "String#capitalize!" do
    it "modifies self in place and returns self" do
      s = "hello"
      assert(s.capitalize!.equal?(s))
      assert_eq(s, "Hello")
    end

    it "returns nil if no changes are made" do
      assert_eq("Hello".capitalize!, nil)
      assert_eq("".capitalize!, nil)
    end

    it "raises a FrozenError on a frozen instance that is modified" do
      s = "hello".freeze
      assert_raises("FrozenError") { s.capitalize! }
    end
  end
end
