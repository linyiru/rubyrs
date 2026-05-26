# `String#each_byte { |b| ... }` — Tier 1 byte iterator. CRuby
# returns an Enumerator when called without a block; rubyrs
# Tier 1 doesn't model Enumerator (ADR 0017 puts it at Tier 2),
# so only the block-given form is wired. The block-less form
# falls through to NoMethodError (same shape as the rest of
# the Enumerator-less iterators).
#
# Documented divergence NOT exercised here: CRuby's `each_byte`
# iterates over the LIVE String, so a block that calls
# `s << "..."` extends the iteration indefinitely (CRuby loops
# until manually broken). rubyrs snapshots the byte buffer at
# call time, so the same code terminates with `count == initial
# bytesize`. The fixture stays off the mutation-during-iteration
# path to keep diff_cruby clean.

# Basic ASCII path.
collected = []
"hello".each_byte { |b| collected << b }
p collected                                  # [104, 101, 108, 108, 111]

# Empty string — block never fires.
fired = false
"".each_byte { |_| fired = true }
puts fired                                   # false

# Return value is the receiver (matches CRuby).
returned = "abc".each_byte { |_| }
puts returned == "abc"                       # true
puts returned.class.name                     # "String"

# Binary bytes (high range) preserved — `each_byte` yields the
# raw byte value, not codepoints.
non = [200, 254, 0, 255].pack("C*")
collected2 = []
non.each_byte { |b| collected2 << b }
p collected2                                 # [200, 254, 0, 255]

# Manual index pattern — `each_byte.with_index` would need an
# Enumerator chain we don't model, so external counter is the
# Tier 1 idiom.
positions = []
i = 0
"abc".each_byte do |b|
  positions << [i, b]
  i += 1
end
p positions                                  # [[0, 97], [1, 98], [2, 99]]

# Build-a-buffer round trip — the canonical pattern that
# motivated this fix (SecureRandom alphanumeric workaround
# used `.bytes.each` before; `.each_byte` is now the idiom).
src = "Hi!"
buf = String.new
src.each_byte { |b| buf << b.chr }
puts buf                                     # "Hi!"
puts buf == src                              # true
