# String#unpack and Array#pack — binary packing/unpacking with
# the subset's directive list:
#   C / c — 8-bit unsigned / signed
#   n / N — 16-bit / 32-bit big-endian unsigned
#   v / V — 16-bit / 32-bit little-endian unsigned
#   q / Q — 64-bit signed / unsigned (native = LE)
#   a / A / Z — raw / space-null-trimmed / null-terminated strings
# Exotic specs (m, U, w, f/d/e/E, etc.) raise ArgumentError —
# documented in SUBSET.md.

# unpack: bytes → Int Array.
puts "ABC".unpack("C*").inspect              # [65, 66, 67]
puts "ABC".unpack("C3").inspect              # [65, 66, 67]
puts "ABC".unpack("c*").inspect              # [65, 66, 67]

# Big-endian / little-endian Int reads.
puts "\x00\x01\x02\x03".unpack("N").inspect  # [66051]
puts "\x00\x01\x02\x03".unpack("n").inspect  # [1]
puts "\x00\x00\x01\x00".unpack("V").inspect  # [65536]
puts "\x00\x01".unpack("v").inspect          # [256]

# String directives.
puts "hello".unpack("a5").inspect            # ["hello"]
puts "hi  ".unpack("A*").inspect             # ["hi"]
puts "x\0\0".unpack("Z*").inspect            # ["x"]
puts "abcd".unpack("a2 a2").inspect          # ["ab", "cd"]

# pack: Array → byte String. Verify via #bytes.
puts [65, 66, 67].pack("C*")                 # ABC
puts [258].pack("N").bytes.inspect           # [0, 0, 1, 2]
puts [256].pack("V").bytes.inspect           # [0, 1, 0, 0]
puts ["AB"].pack("a4").bytes.inspect         # [65, 66, 0, 0]
puts ["AB"].pack("A4").bytes.inspect         # [65, 66, 32, 32]
puts ["AB"].pack("Z4").bytes.inspect         # [65, 66, 0, 0]

# Round-trip.
roundtrip = [1, 2, 3, 4].pack("N*").unpack("N*")
puts roundtrip.inspect                       # [1, 2, 3, 4]

# 64-bit Q is native (LE). Using a value within Int32 to dodge
# a pre-existing 64-bit-literal parse limitation (separately
# tracked).
puts [65537].pack("Q").bytes.inspect         # [1, 0, 1, 0, 0, 0, 0, 0]
