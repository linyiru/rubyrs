# Adapted from ruby/spec core/string/swapcase_spec.rb at
# upstream commit 448cb340 (2026-05). Hand-translated —
# baseline ASCII case-flip shape. The Unicode case-mapping
# options (`swapcase(:turkic)` / `:lithuanian` / `:fold` /
# `:ascii`) are dropped (Tier-2 Encoding work — ADR 0020).

describe "String#swapcase" do
  it "returns a copy with each ASCII letter's case flipped" do
    assert_eq("Hello World".swapcase, "hELLO wORLD")
    assert_eq("HELLO".swapcase, "hello")
    assert_eq("hello".swapcase, "HELLO")
  end

  it "returns an empty String for an empty receiver" do
    assert_eq("".swapcase, "")
  end

  it "leaves non-letter chars unchanged" do
    assert_eq("Hi!".swapcase, "hI!")
    assert_eq("123".swapcase, "123")
    assert_eq("a1B2c3".swapcase, "A1b2C3")
  end

  it "returns a new string (non-destructive)" do
    s = "Hello"
    assert_eq(s.swapcase, "hELLO")
    assert_eq(s, "Hello")
  end

  it "raises ArgumentError on any positional arg (option forms unsupported)" do
    # CRuby's `String#swapcase` takes an optional Unicode
    # case-mapping option symbol — we don't implement the
    # option form (ADR 0020 Tier-2 Encoding) so any arg
    # surfaces as `wrong number of arguments` rather than
    # falling through to NoMethodError.
    assert_raises("ArgumentError") { "Hi".swapcase(:ascii) }
    assert_raises("ArgumentError") { "Hi".swapcase(:turkic) }
  end

  # skipped (method-not-implemented): it "respects the Unicode case-mapping options" do
  #   Option forms `swapcase(:ascii)` / `:turkic` /
  #   `:lithuanian` / `:fold`. Tier-2 Encoding work (ADR 0020).
  describe "String#swapcase!" do
    it "modifies self in place and returns self" do
      s = "Hello"
      assert(s.swapcase!.equal?(s))
      assert_eq(s, "hELLO")
    end

    it "returns nil if no changes are made" do
      assert_eq("123".swapcase!, nil)
      assert_eq("".swapcase!, nil)
    end

    it "raises a FrozenError on a frozen instance that is modified" do
      s = "Hello".freeze
      assert_raises("FrozenError") { s.swapcase! }
    end
  end
end
