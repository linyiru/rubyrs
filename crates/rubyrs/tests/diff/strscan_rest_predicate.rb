# StringScanner#rest? — true while there's unscanned input (inverse of
# eos?). tzinfo's POSIX TZ parser loops `while scanner.rest?`.
require "strscan"
s = StringScanner.new("ab")
p s.rest?        # true
p s.eos?         # false
s.scan(/a/)
p s.rest?        # true
s.scan(/b/)
p s.rest?        # false
p s.eos?         # true
s2 = StringScanner.new("")
p s2.rest?       # false
