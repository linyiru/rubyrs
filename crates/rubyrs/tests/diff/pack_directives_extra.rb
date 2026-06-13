# Pack/unpack directive expansion beyond A6a — signed
# fixed-width integers (`s` / `l` with optional `<` / `>`)
# and hex strings (`H` / `h`). These were left out of the
# original A6a wave because msgpack's BigInt path only
# needed unsigned + endian-modified `L`/`Q`; this fixture
# closes the gap for binary-protocol parsers that read or
# emit signed ints / hex digests.
#
# Inputs are constructed via `pack` (raw-byte safe) rather
# than `"\xNN"` string literals because rubyrs's string
# lexer routes high-byte escapes through UTF-8 substitution
# (a separate gap, documented in SUBSET.md).

# --- Signed 16-bit, both endians ---
puts [-1, -32768, 32767].pack("s<*").bytes.inspect   # LE
puts [-1, -32768, 32767].pack("s>*").bytes.inspect   # BE

# Round-trip: pack as signed, unpack as signed, get original.
[-1, -32768, 32767, 100, -100].each do |v|
  le = [v].pack("s<")
  be = [v].pack("s>")
  puts le.unpack1("s<")
  puts be.unpack1("s>")
end

# --- Signed 32-bit, both endians ---
puts [-1, -2147483648, 2147483647].pack("l<*").bytes.inspect
puts [-1, -2147483648, 2147483647].pack("l>*").bytes.inspect

[-1, -2147483648, 2147483647, 1234567890, -987654321].each do |v|
  le = [v].pack("l<")
  be = [v].pack("l>")
  puts le.unpack1("l<")
  puts be.unpack1("l>")
end

# --- Hex strings, high nibble first (`H`) and low nibble
#     first (`h`) ---
#
# Pack: hex digits → bytes.
puts ["abcd"].pack("H*").bytes.inspect    # [0xab, 0xcd]
puts ["abcd"].pack("h*").bytes.inspect    # nibble-reversed
puts ["deadbeef"].pack("H*").bytes.inspect
puts ["deadbeef"].pack("h*").bytes.inspect

# Round-trip via pack→unpack (no high-byte literal needed).
[
  "00",
  "ff",
  "abcd",
  "deadbeef",
  "0123456789abcdef",
].each do |hex|
  bytes = [hex].pack("H*")
  puts bytes.unpack1("H*")               # back to original
end

# Odd-length hex: trailing nibble pads with 0 on the right.
puts ["abc"].pack("H*").bytes.inspect    # [0xab, 0xc0]

# `*` vs explicit count.
puts "\x12\x34\x56\x78".unpack1("H4")    # only 4 nibbles

# --- Base64 ('m' = RFC2045: newline every 60 chars + trailing
#     newline; 'm0' = RFC4648: no breaks). rack's basic-auth reader
#     does `creds.unpack1('m')`; its test builds the header with
#     `[user_pass].pack('m*')`. ---
puts ["user:pass"].pack("m")             # "dXNlcjpwYXNz\n"
puts ["user:pass"].pack("m*")            # same as m
puts ["user:pass"].pack("m0")            # no trailing newline
puts ["Hello, World!"].pack("m0")
puts [("a" * 50)].pack("m").inspect      # 60-char wrap + trailing \n
puts "dXNlcjpwYXNz\n".unpack1("m")       # "user:pass"
puts "dXNlcjpwYXNz".unpack("m").inspect  # ["user:pass"]
# whitespace/newlines are ignored on decode (RFC2045).
puts "dXNl\ncjpw YXNz".unpack1("m")      # "user:pass"

# Round-trip across lengths + the empty/edge cases, both modes.
["", "a", "ab", "abc", "abcd", "any carnal pleasure.", "x" * 100].each do |s|
  puts([s].pack("m").unpack1("m") == s)
  puts([s].pack("m0").unpack1("m") == s)
end
