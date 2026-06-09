# String#partition / #rpartition (string + regex separators),
# #insert (char-indexed, negative-from-end), #delete (tr-style set).

# partition — first occurrence
p "a-b-c".partition("-")
p "key=value=x".partition("=")
p "abc".partition("-")          # no match → [self, "", ""]
p "".partition("x")

# rpartition — last occurrence
p "a-b-c".rpartition("-")
p "abc".rpartition("-")         # no match → ["", "", self]

# regex separators
p "hello".partition(/l+/)
p "hello".rpartition(/l/)
p "a1b2c3".partition(/\d/)
p "a1b2c3".rpartition(/\d/)
p "xyz".partition(/\d/)         # regex no match

# insert — char-indexed
p "hello".insert(0, ">")
p "hello".insert(2, "XX")
p "hello".insert(5, "!")        # at end
p "hello".insert(-1, "!")       # append (negative)
p "hello".insert(-3, "_")

# delete — tr-style set
p "hello".delete("l")
p "hello world".delete("lo")
p "abcdef".delete("a-c")        # range
p "hello".delete("^l")          # negation
p "hello".delete("z")           # nothing matches

# chaining
p "a,b,c,d".rpartition(",").first
p "  trim me  ".strip.partition(" ")
