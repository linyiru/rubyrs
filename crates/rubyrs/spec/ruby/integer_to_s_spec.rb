# Adapted from ruby/spec core/integer/to_s_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-polished:
# - `.should ==` → `assert_eq`; `should.raise(X)` → `assert_raises`.
# - `bignum_value` → `(2**64)`; `bignum_value(N)` → `(2**64 + N)`.
# - upstream nests `context "fixnum"/"bignum"` × `context "when
#   given a base"/"when no base given"`; the micro-runner doesn't
#   define `context`, so flatten both layers into descriptive
#   top-level it / bignum_it names.
# - skipped (fixture): the `before :each` / `after :each` hook
#   pair around `Encoding.default_internal` — the micro-runner
#   has no hook lift, and the four encoding `it` blocks they
#   bracket are skipped below for `divergent` reasons anyway.
# - skipped (divergent): the four "returns a String in US-ASCII
#   encoding" tests. rubyrs's String stores UTF-8 only; the
#   encoding-stub returned by `String#encoding` reports "UTF-8"
#   and doesn't `.equal?(Encoding::US_ASCII)`. Implementing
#   CRuby's per-method encoding contract requires Tier-2
#   Encoding work (ADR 0020); out of B.6 scope.

describe "Integer#to_s" do
  it "fixnum: with base returns self converted to a String in the given base" do
    assert_eq(12345.to_s(2), "11000000111001")
    assert_eq(12345.to_s(8), "30071")
    assert_eq(12345.to_s(10), "12345")
    assert_eq(12345.to_s(16), "3039")
    assert_eq(95.to_s(16), "5f")
    assert_eq(12345.to_s(36), "9ix")
  end

  it "fixnum: with base raises ArgumentError if the base is less than 2 or higher than 36" do
    assert_raises("ArgumentError") { 123.to_s(-1) }
    assert_raises("ArgumentError") { 123.to_s(0) }
    assert_raises("ArgumentError") { 123.to_s(1) }
    assert_raises("ArgumentError") { 123.to_s(37) }
  end

  bignum_it "bignum: with base returns self converted to a String using the given base" do
    a = 2**64
    assert_eq(a.to_s(2),
      "10000000000000000000000000000000000000000000000000000000000000000")
    assert_eq(a.to_s(8), "2000000000000000000000")
    assert_eq(a.to_s(16), "10000000000000000")
    assert_eq(a.to_s(32), "g000000000000")
  end

  bignum_it "bignum: with base raises ArgumentError if the base is less than 2 or higher than 36" do
    bn = 2**64
    assert_raises("ArgumentError") { bn.to_s(-1) }
    assert_raises("ArgumentError") { bn.to_s(0) }
    assert_raises("ArgumentError") { bn.to_s(1) }
    assert_raises("ArgumentError") { bn.to_s(37) }
  end

  it "fixnum: with no base returns self converted to a String using base 10" do
    assert_eq(255.to_s, "255")
    assert_eq(3.to_s, "3")
    assert_eq(0.to_s, "0")
    assert_eq((-9002).to_s, "-9002")
  end

  bignum_it "bignum: with no base returns self converted to a String using base 10" do
    assert_eq((2**64 + 9).to_s, "18446744073709551625")
    assert_eq((2**64).to_s, "18446744073709551616")
    assert_eq((-(2**64 + 675)).to_s, "-18446744073709552291")
  end

  # skipped (fixture): `before :each` / `after :each` hook pair
  # around `Encoding.default_internal` — the four encoding
  # blocks they bracket are themselves skipped below.

  # skipped (divergent): rubyrs's String stores UTF-8 only; the
  # encoding-stub returned by String#encoding reports "UTF-8"
  # and doesn't .equal?(Encoding::US_ASCII). Tier-2 Encoding
  # work (ADR 0020) is out of B.6 scope. Bracketed by the
  # before/after fixture above.
  #
  # it "fixnum: returns a String in US-ASCII encoding when Encoding.default_internal is nil" do
  #   Encoding.default_internal = nil
  #   assert(1.to_s.encoding.equal?(Encoding::US_ASCII))
  # end
  #
  # it "fixnum: returns a String in US-ASCII encoding when Encoding.default_internal is not nil" do
  #   Encoding.default_internal = Encoding::IBM437
  #   assert(1.to_s.encoding.equal?(Encoding::US_ASCII))
  # end
  #
  # bignum_it "bignum: returns a String in US-ASCII encoding when Encoding.default_internal is nil" do
  #   Encoding.default_internal = nil
  #   assert((2**64).to_s.encoding.equal?(Encoding::US_ASCII))
  # end
  #
  # bignum_it "bignum: returns a String in US-ASCII encoding when Encoding.default_internal is not nil" do
  #   Encoding.default_internal = Encoding::IBM437
  #   assert((2**64).to_s.encoding.equal?(Encoding::US_ASCII))
  # end
end
