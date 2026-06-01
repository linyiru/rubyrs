# String#split with the 2-arg (sep, limit) form, plus the no-limit
# trailing-empty-field semantics. CRuby is the oracle.

# limit > 0: at most `limit` fields, last holds the remainder.
puts "a=b=c".split("=", 2).inspect          # ["a", "b=c"]
puts "k=v=w=x".split("=", 3).inspect         # ["k", "v", "w=x"]
puts "one two three".split(" ", 2).inspect   # ["one", "two three"]

# limit larger than the number of fields: just splits normally.
puts "a,b".split(",", 5).inspect             # ["a", "b"]

# limit < 0: split fully, KEEP trailing empty fields.
puts "a,b,,".split(",", -1).inspect          # ["a", "b", "", ""]

# limit == 0: like no-limit — DROP trailing empty fields.
puts "a,b,,".split(",", 0).inspect           # ["a", "b"]

# no limit: trailing empty fields dropped; interior ones kept.
puts "a,b,,".split(",").inspect              # ["a", "b"]
puts "a,,b,".split(",").inspect              # ["a", "", "b"]
puts ",,".split(",").inspect                 # []

# empty separator: per-character, limit caps with a joined remainder.
puts "abcde".split("", 3).inspect            # ["a", "b", "cde"]
puts "abc".split("", -1).inspect             # ["a", "b", "c"]

# the canonical key=value parse (the idiom GAP #9 was blocking).
k, v = "name=Ada=Lovelace".split("=", 2)
puts "#{k} -> #{v}"                            # name -> Ada=Lovelace
