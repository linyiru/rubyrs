# String literal high-byte preservation — `"\xFF\xFF"` and
# other `\xNN` escapes producing non-UTF-8 byte sequences
# previously expanded each invalid byte to the 3-byte U+FFFD
# replacement (`\xEF\xBF\xBD`), breaking any binary-protocol
# parser that needed raw `\x80..\xFF` bytes from a literal.
#
# Now stored as raw bytes through a per-Proto `byte_literals`
# pool (the global interner is UTF-8-only). Valid-UTF-8
# literals still hit the interner's fast path. Mixed-encoding
# literals (`"hello\xFF"`) flow through the byte path.

# --- High-byte literals: 2-byte raw input ---
puts "\xFF\xFF".bytes.inspect     # [255, 255]
puts "\xFF\xFF".bytesize          # 2 (not 6)

# CRuby's String#length on a "\xFF\xFF" literal counts the
# string's encoding-aware chars; the literal is tagged
# ASCII-8BIT so length == 2 (one byte = one "char"). rubyrs
# doesn't model encoding tags and falls back to UTF-8 char
# count via from_utf8_lossy — each invalid byte becomes one
# U+FFFD "char". The byte content is correct now (`bytes`
# returns the raw bytes); the per-char `length` is the
# documented Tier-1 divergence and is omitted from the
# fixture rather than asserted.

# --- Various high-byte patterns ---
puts "\x80".bytes.inspect         # [128]
puts "\x81\x82\x83".bytes.inspect # [129, 130, 131]
puts "\xAB\xCD".unpack1("H*")     # "abcd" — pack-engine hex now works

# --- Mixed printable + high bytes ---
puts "hi\xFF".bytes.inspect       # [104, 105, 255]
puts "\x00abc\xFF".bytes.inspect  # [0, 97, 98, 99, 255]
puts "\xFF\xFF\xFF\xFF".unpack1("N")  # 4294967295

# --- Literal vs pack equivalence: same bytes either way ---
puts "\xFF\xFF\xFF\xFF".bytes == [0xFF, 0xFF, 0xFF, 0xFF].pack("C*").bytes  # true
puts "\x80\x81\x82\x83".bytes == [0x80, 0x81, 0x82, 0x83].pack("C*").bytes  # true
puts "\x00\xFF".bytes         == [0x00, 0xFF].pack("C*").bytes              # true

# --- Valid-UTF-8 literals stay on the interner fast path ---
puts "hello".bytes.inspect        # [104, 101, 108, 108, 111]
puts "中文".bytes.inspect         # [228, 184, 173, 230, 150, 135]
puts "ASCII only".bytesize        # 10

# --- Pack output as a literal-compatible String ---
# Verify a binary literal can be passed to unpack just like
# a packed value would be.
puts "\x12\x34\x56\x78".unpack1("N")  # 305419896

# --- Signed-int round-trip via literal input ---
puts "\xFF\xFF".unpack1("s<")     # -1
puts "\xFF\xFF\xFF\xFF".unpack1("l<")  # -1
