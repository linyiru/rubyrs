# `String#ord` — Integer codepoint of the first character, in the
# receiver's encoding. ArgumentError on an empty string. A BINARY
# string yields its first byte. rack's CommonLogger escapes
# non-printables via `msg.gsub!(/[^[:print:]]/) { |c| sprintf("\\x%x",
# c.ord) }`.
p "A".ord            # 65
p "abc".ord          # 97 (first char only)
p "\x1f".ord         # 31  (control char)
p "\n".ord           # 10
p "0".ord            # 48
p "é".ord            # 233 (UTF-8 scalar)
p "€".ord            # 8364
p "\xab".b.ord       # 171 (binary: first byte)
p "\xff\x00".b.ord   # 255

# the CommonLogger escaping shape
p "GET\x1f".gsub(/[^[:print:]]/) { |c| sprintf("\\x%x", c.ord) }

begin
  "".ord
rescue ArgumentError => e
  puts "ArgumentError: #{e.message}"
end
