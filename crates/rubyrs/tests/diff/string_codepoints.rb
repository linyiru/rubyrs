# String#codepoints / #each_codepoint — integer Unicode code points
# per character. A BINARY subject yields raw byte values.
p "abc".codepoints
p "héllo".codepoints
p "".codepoints
p "abc".b.codepoints

# Block form yields each code point; returns the receiver.
acc = []
ret = "héllo".each_codepoint { |cp| acc << cp }
p acc
p ret == "héllo"

# No-block form returns an Enumerator.
p "abc".each_codepoint.to_a
p "héllo".each_codepoint.to_a
p "abc".each_codepoint.with_index.to_a

# break value propagates from the block.
p("abcd".each_codepoint { |cp| break cp if cp == 99 })
