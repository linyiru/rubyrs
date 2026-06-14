# String#match / Regexp#match on an ASCII-8BIT subject must run the byte
# engine: a lossy UTF-8 bound both BREAKS byte-level patterns (e.g.
# `/\xC3/n`) and U+FFFD-mangles captures. The match must succeed and the
# numbered captures come back byte-faithful + ASCII-8BIT. (Completes the
# binary-capture surface alongside =~ / $~ / StringScanner.)
# NB: $~[0]/whole-via-MatchData stays lossy for binary — a documented
# edge; $& (step.rs) and the positional captures [1..] are faithful.

s = "x\xC3y".b
m = s.match(/(.)\xC3(.)/n)
p m.nil?                          # false — byte pattern MATCHES (was nomatch)
p [m[1].bytes, m[2].bytes]        # [[120], [121]]
p [m[1].encoding.to_s, m[2].encoding.to_s]   # ["ASCII-8BIT", "ASCII-8BIT"]

# capture containing an invalid byte (rack multipart filename shape)
m2 = "name=\"inv\xC3.txt\"".b.match(/name="(.*?)"/)
p m2[1].bytes                     # inv\xC3.txt bytes (195 preserved, not 239,191,189)
p m2[1].encoding.to_s             # "ASCII-8BIT"

# Regexp#match (receiver form), symmetric
p Regexp.new("(.)(.)".b).match("a\xFF".b).captures.map(&:bytes)   # [[97],[255]]

# no-match returns nil
p "xyz".b.match(/(\d)/n)          # nil

# valid-content binary still works
p "abc".b.match(/(b)(c)/).captures   # ["b", "c"]
