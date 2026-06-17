# Integer#ord returns self (an integer is its own codepoint).
# regexp_parser's scanner calls `.ord` on already-integer bytes.
p 65.ord            # 65
p 0.ord             # 0
p(-5.ord)           # -5
p "A".bytes.first.ord  # 65
p [104, 105].map(&:ord)  # [104, 105]
