# Adapted from ruby/spec core/string/swapcase_spec.rb at
# upstream commit 448cb340 (2026-05). Hand-translated —
# baseline ASCII case-flip shape.
#
# Two related Unicode gaps are out of subset (both gated on
# ADR 0020 Tier-2 Encoding work):
#
#   1. **Option form** — `swapcase(:turkic)` /
#      `:lithuanian` / `:fold` / `:ascii`. Surfaces as
#      ArgumentError here; covered by the wrong-arity test.
#   2. **Default behaviour on non-ASCII letters** — CRuby
#      since 2.4 has been Unicode-aware in the no-option
#      form (`"Café".swapcase == "cAFÉ"`). Here the
#      ASCII-only flip leaves non-ASCII letters unchanged
#      (`"Café".swapcase == "cAFé"`). Pinned by the
#      "leaves non-ASCII letters unchanged" example below
#      so the gap is intentional, witnessed, and obvious.

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

  it "leaves non-ASCII letters unchanged (ASCII-only flip per ADR 0020)" do
    # Divergent from CRuby ≥ 2.4 (Unicode-aware default).
    # CRuby returns "cAFÉ" / "üBER" (both case-flipped);
    # we leave the non-ASCII letters untouched and only
    # flip the ASCII letters around them. Pinning the gap
    # so a future Tier-2 Encoding patch knows which
    # assertions to flip.
    assert_eq("Café".swapcase, "cAFé")    # CRuby: "cAFÉ"
    assert_eq("Über".swapcase, "ÜBER")    # CRuby: "üBER"
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
