# StringScanner surface beyond scan/[]: check/skip/match?/scan_until/
# check_until/pre_match/post_match/matched/pos=/getch/bol?.
require "strscan"
s = StringScanner.new("hello world 123")
p s.scan(/\w+/)
p s.check(/\s/)        # no advance
p s.pos
p s.skip(/\s/)         # returns matched length, advances
p s.scan_until(/\d+/)
p s.pre_match
p s.matched
p s.post_match
p s.eos?
t = StringScanner.new("ab\ncd")
p t.getch
p t.bol?
t.scan(/b/); t.getch
p t.bol?
