# An ASCII-8BIT (BINARY) string counts every byte as one char
# (`length == bytesize`), even when the bytes don't form valid UTF-8 —
# CRuby semantics. Treating a binary buffer as UTF-8 under-counts and
# desyncs byte-offset arithmetic (rack reads multipart bodies as binary
# via StringIO and advances scanner positions with `length`).

bin = ("\xC3\xC3\xFF\x80hello" * 4).b
puts "bytesize=#{bin.bytesize} length=#{bin.length}"   # equal
puts "size=#{bin.size}"

# slicing a binary string is byte-indexed and stays byte-faithful
sl = bin[0, 10]
puts "slice bytesize=#{sl.bytesize} length=#{sl.length} enc=#{sl.encoding}"

# String#replace adopts the source string's encoding
buf = String.new            # UTF-8
buf.replace("\xC3\x28".b)   # ASCII-8BIT source
puts "replace enc=#{buf.encoding} bytesize=#{buf.bytesize}"
