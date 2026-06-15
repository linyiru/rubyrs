# String#chop — drop a trailing \r\n pair, else the last character
# (UTF-8 multibyte aware); empty string stays empty; non-mutating.
# Surfaced by net/protocol's readline (`readuntil("\n").chop`).
p "hello".chop
p "hello\n".chop
p "hello\r\n".chop
p "hello\r".chop
p "".chop
p "a".chop
p "café".chop          # drops the full é (2 bytes)
p "café\r\n".chop      # \r\n wins over the char
p "snowman☃".chop      # 3-byte char
p "x".chop.chop        # chained, underflow-safe
# Non-mutating: receiver unchanged.
s = "abc"
t = s.chop
p [s, t]
