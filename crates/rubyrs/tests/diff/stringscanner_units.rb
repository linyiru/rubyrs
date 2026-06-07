# StringScanner#getch sets the match register to the consumed char (not
# nil), #pos= range-checks like CRuby, and pre_match/post_match are
# correct on multibyte input (the =~ char-index fix).
require "strscan"

s = StringScanner.new("hello")
p s.getch          # "h"
p s[0]             # "h"
p s.matched        # "h"
p s.matched?       # true
p s.pre_match      # ""
p s.post_match     # "ello"
p s.matched_size   # 1
p s.getch          # "e"
p s.pre_match      # "h"
p s.post_match     # "llo"

s2 = StringScanner.new("abc")
s2.pos = 2
p s2.pos           # 2
begin; s2.pos = 10; rescue RangeError => e; puts "RangeError: #{e.message}"; end
begin; s2.pos = -10; rescue RangeError => e; puts "RangeError: #{e.message}"; end
p s2.pos           # 2 (unchanged after the failed assignments)

# Multibyte pre_match / post_match.
s3 = StringScanner.new("αβ-γδ")
s3.scan_until(/-/)
p s3.pre_match     # "αβ"
p s3.post_match    # "γδ"
p s3.matched       # "-"
