# String#[] / #slice on a UTF-8-tagged-but-INVALID string must index by
# UTF-8 character boundaries (a valid sequence is one char; each invalid
# byte is one char) while preserving the EXACT bytes — CRuby's lenient
# behaviour. The lossy path (to_string_lossy) expands every invalid byte
# to a 3-byte U+FFFD, which corrupts AND grows the slice. rack reads
# multipart bodies as UTF-8-tagged binary (`File.read`), and StringIO#read
# slices them with `str[pos, len]`.

s = "abc\xC3\xC3\xFFdef\xFFghi"   # UTF-8 literal with invalid bytes (stays UTF-8)
puts "len=#{s.length} bytesize=#{s.bytesize}"

# single-char index
puts "s[0]=#{s[0]} s[3].bytesize=#{s[3].bytesize}"

# (start, len)
puts "s[0,8].bytesize=#{s[0,8].bytesize}"
puts "s[3,3].bytesize=#{s[3,3].bytesize}"
puts "s[5,100].bytesize=#{s[5,100].bytesize}"

# ranges (incl. endless / beginless)
puts "s[3..].bytesize=#{s[3..].bytesize}"
puts "s[..4].bytesize=#{s[..4].bytesize}"
puts "s[2..6].bytesize=#{s[2..6].bytesize}"
puts "s[2...6].bytesize=#{s[2...6].bytesize}"
puts "s[-3..].bytesize=#{s[-3..].bytesize}"

# slices preserve bytes exactly (round-trip the leading ASCII part)
puts "s[0,3]=#{s[0,3]}"

# valid multibyte UTF-8 mixed with invalid: each valid char is one unit
m = "aé\xFFb"               # 'a', 'é' (2 bytes), invalid 0xFF, 'b'
puts "m.length=#{m.length} m[1].bytesize=#{m[1].bytesize} m[0,2].bytesize=#{m[0,2].bytesize}"

# out-of-range
p s[100]
p s[100, 5]
