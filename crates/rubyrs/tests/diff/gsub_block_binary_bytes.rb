# `String#gsub` / `#sub` with a block that returns a binary
# (non-UTF-8) String must splice those bytes into the result
# verbatim. Pre-fix, rubyrs accumulated the result as a Rust
# `String` and ran every block return through a lossy UTF-8
# decode — invalid bytes (e.g. 0xE4 returned by
# `[228].pack('C')`) were rewritten to `U+FFFD` (3 bytes),
# corrupting multi-byte sequences like `%E4%B8%AD` (中) into
# `���`. The fix accumulates as `Vec<u8>` and copies the
# RStr's raw bytes for `Value::Str` block returns.
#
# This fixture covers the percent-decode pattern that
# URI::DEFAULT_PARSER#unescape and many URL/HTML decoders use,
# plus a few adjacent shapes (single-byte pack, multi-byte
# UTF-8 char in the input, mixed ASCII/encoded).

# 1. Single percent-escape decoded to one byte, then printed by
#    its byte sequence — should be a single 0xE4 byte, not the
#    3-byte U+FFFD replacement.
puts "%E4".gsub(/%([0-9A-Fa-f]{2})/) { [$1.to_i(16)].pack('C') }.bytes.inspect

# 2. Full UTF-8 character `中` round-trips through percent-encode
#    + percent-decode and comes out byte-identical. This is the
#    motivating case for the URI shim.
encoded = "中".bytes.map { |b| "%%%02X" % b }.join
decoded = encoded.gsub(/%([0-9A-Fa-f]{2})/) { [$1.to_i(16)].pack('C') }
puts decoded.bytes.inspect

# 3. Mixed ASCII + percent-escapes — literal segments pass
#    through untouched, encoded segments decode to raw bytes.
mixed = "hello%20world%2F中".gsub(/%([0-9A-Fa-f]{2})/) do
  [$1.to_i(16)].pack('C')
end
puts mixed.bytes.inspect

# 4. `sub` (single-replace) variant of the same byte-output
#    contract.
puts "%C3%A9".sub(/%([0-9A-Fa-f]{2})/) { [$1.to_i(16)].pack('C') }.bytes.inspect

# 5. Block returns a multi-byte binary string built from
#    several `pack('C')` concatenations.
multibyte = "X".gsub(/X/) { [228, 184, 173].pack('C*') }
puts multibyte.bytes.inspect
