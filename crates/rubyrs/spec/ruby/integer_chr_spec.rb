# Adapted from ruby/spec core/integer/chr_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-polished:
# - `.should ==` → `assert_eq`; `should raise_error` → `assert_raises`.
# - upstream wraps cases in `context "without argument"` and
#   `context "with an encoding argument"`; the micro-runner has
#   no `context`, so flatten into descriptive top-level its.
# - `bignum_value` → `(2**64)`; bignum cases gated on `bignum_it`.
# - skipped (divergent): rubyrs's `String#encoding` returns a
#   stub reporting "UTF-8" rather than an Encoding object that
#   `.equal?(Encoding::US_ASCII)`, so the upstream "returns a
#   String in US-ASCII encoding" / "...BINARY encoding" /
#   "...UTF-8 encoding" assertions can't be replayed faithfully.
#   Tier-2 Encoding work (ADR 0020) is the prerequisite.
# - skipped (method-not-implemented): the `chr(encoding)` form
#   widens the accepted range to U+10FFFF and tags the result
#   with the requested Encoding. We don't model the Encoding
#   object yet, so the 1-arg form raises TypeError
#   ("no implicit conversion of X into Encoding") for any arg.
#   Cases that rely on the wider range — `0x80.chr(Encoding::UTF_8)`,
#   `256.chr(Encoding::UTF_8)`, etc. — are skipped.

describe "Integer#chr" do
  it "returns a String containing the ASCII character for the receiver" do
    assert_eq(65.chr, "A")
    assert_eq(97.chr, "a")
    assert_eq(48.chr, "0")
  end

  it "returns a single-byte String for the 0..127 range" do
    assert_eq(0.chr, "\x00")
    assert_eq(1.chr, "\x01")
    assert_eq(127.chr, "\x7f")
  end

  it "returns a single-byte String for the 128..255 range" do
    assert_eq(128.chr.bytes, [128])
    assert_eq(200.chr.bytes, [200])
    assert_eq(255.chr.bytes, [255])
  end

  it "returns a String of length 1" do
    assert_eq(0.chr.length, 1)
    assert_eq(65.chr.length, 1)
    assert_eq(255.chr.length, 1)
  end

  it "raises a RangeError if self is less than 0" do
    assert_raises("RangeError") { (-1).chr }
    assert_raises("RangeError") { (-100).chr }
  end

  it "raises a RangeError if self is greater than 255 without an encoding" do
    assert_raises("RangeError") { 256.chr }
    assert_raises("RangeError") { 1000.chr }
  end

  bignum_it "bignum: raises a RangeError because the receiver is out of char range" do
    assert_raises("RangeError") { (2**64).chr }
    assert_raises("RangeError") { (-(2**64)).chr }
  end

  it "raises a TypeError when given a non-Encoding argument" do
    # `Integer#chr(encoding)` is unsupported (see header). Any arg
    # surfaces TypeError, matching CRuby's shape for non-Encoding
    # input (e.g. `42.chr("UTF-8")` → "no implicit conversion of
    # String into Encoding").
    assert_raises("TypeError") { 65.chr("UTF-8") }
    assert_raises("TypeError") { 65.chr(nil) }
  end

  # skipped (method-not-implemented): `chr(encoding)` widens the
  # accepted range to U+10FFFF and tags the result with the
  # requested Encoding. Needs Tier-2 Encoding work (ADR 0020).
  #
  # it "returns a multi-byte String for 0x80..U+10FFFF given Encoding::UTF_8" do
  #   assert_eq(0x80.chr(Encoding::UTF_8).bytes, [0xc2, 0x80])
  # end

  # skipped (divergent): rubyrs's `String#encoding` returns a
  # stub that doesn't `.equal?(Encoding::US_ASCII)` /
  # `.equal?(Encoding::BINARY)`. ADR 0020 prerequisite.
  #
  # it "returns a String in US-ASCII encoding when self is 0..127" do
  #   assert(65.chr.encoding.equal?(Encoding::US_ASCII))
  # end
  #
  # it "returns a String in BINARY encoding when self is 128..255" do
  #   assert(128.chr.encoding.equal?(Encoding::BINARY))
  # end
end
