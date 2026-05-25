# String#[] / String#slice — char-indexed subscript & slicing.
# Supported forms:
#   s[i]          → single-char String or nil
#   s[i, n]       → substring of n chars starting at i, nil if i OOB
#   s[range]      → substring; nil for invalid start
# Negative indices count from the end.

s = "hello world"

# Single index — positive, zero, negative.
p s[0]
p s[4]
p s[10]
p s[-1]
p s[-5]
p s[-11]

# Out of bounds → nil.
p s[100]
p s[-100]

# Two-arg slice — start + length.
p s[0, 5]
p s[6, 5]
p s[0, 100]    # length clamped to remaining
p s[6, 0]      # empty
p s[11, 0]     # at end, empty
p s[11, 5]     # at end with length
p s[12, 1]     # past end, nil
p s[-5, 5]
p s[-11, 5]

# Range slice — inclusive/exclusive, negative bounds.
p s[0..4]
p s[6..10]
p s[0...5]
p s[6...11]
p s[-5..-1]
p s[-5...-1]
p s[0..100]    # over-long range clamps

# slice alias.
p s.slice(6, 5)
p s.slice(0, 5)
p s.slice(0)
p s.slice(0..4)

# Edge cases.
p ""[0]
p ""[0, 0]
p "x"[0]
p "x"[1]
p "x"[-1]
p "x"[-2]

# Inside expressions.
def first_word(text)
  idx = text.index(" ")
  return text if idx.nil?
  text[0, idx]
end
puts first_word("hello world")
puts first_word("nope")

# Method chains.
puts "Hello, World!"[7, 5].upcase
puts "abcdef"[2..4]

# In iteration.
"hello".chars.each_with_index do |_c, i|
  puts "hello"[i]
end
