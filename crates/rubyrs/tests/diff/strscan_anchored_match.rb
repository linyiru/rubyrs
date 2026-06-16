# StringScanner anchored match (scan/check/skip/match?) — exercises both
# the native byte engine and the fancy (lookaround/backref) anchored
# fallback, plus `$~` / captures after a scan, on an ASCII buffer (the
# byte-addressable fast path). Guards the do_strscan_match_at_binary
# rewrite (no per-call tail copy, no forward scan).
require "strscan"

s = StringScanner.new("foo123 bar(?baz)")
p s.scan(/\w+/)          # "foo123"  (native)
p s.check(/\s/)          # " "       (native, non-advancing)
p s.pos                  # 6         (check didn't move)
p s.skip(/\s+/)          # 1
p s.scan(/\w+/)          # "bar"
p s.matched              # "bar"

# Named + numbered captures via scan are readable through the scanner's
# own match register (StringScanner does NOT set the global $~).
s2 = StringScanner.new("2026-06-16 rest")
p s2.scan(/(?<y>\d{4})-(?<m>\d\d)-(?<d>\d\d)/)  # "2026-06-16"
p s2[:y]                 # "2026"
p s2[:m]                 # "06"
p s2[1]                  # "2026" (numbered)
p s2.post_match          # " rest"

# Fancy pattern (lookahead) anchored at pos — must take the fancy
# fallback and still match only AT the current position.
s3 = StringScanner.new("abcabc")
p s3.scan(/a(?=b)/)      # "a" (lookahead)
p s3.scan(/bc/)          # "bc"
p s3.check(/(?=x)/)      # nil  (lookahead fails -> no anchored match)
p s3.pos                 # 3

# match? returns the matched length without advancing.
s4 = StringScanner.new("hello")
p s4.match?(/he/)        # 2
p s4.pos                 # 0

# A miss at the current position returns nil (anchored, not a search).
s5 = StringScanner.new("xyzabc")
p s5.scan(/abc/)         # nil — "abc" is ahead, not at pos 0
p s5.pos                 # 0
