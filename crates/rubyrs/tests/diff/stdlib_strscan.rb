# StringScanner — pure-Ruby vendor under `--features stdlib`.
#
# CRuby's strscan is a C extension. Tilt's ERB compilation path
# (lib/erb/compiler.rb:238 — `StringScanner.new(@src)` inside
# both SimpleScanner and ExplicitScanner) is the motivating
# consumer. Without this vendor, ERB load surfaces
# "cannot find C ext: strscan".
#
# This fixture pins the API surface ERB actually exercises:
# `new`, `eos?`, `scan(regex)`, `[](n)`, plus the few utility
# methods (`rest`, `pos`, `peek`) included for completeness.

require "strscan"

# --- new + initial state ---
s = StringScanner.new("hello world")
puts s.class                                    # StringScanner
puts s.pos                                      # 0
puts s.eos?                                     # false
puts s.rest                                     # hello world

# --- scan: anchored hit, advances pos ---
puts s.scan(/hello/).inspect                    # "hello"
puts s.pos                                      # 5
puts s.eos?                                     # false

# --- scan: anchored miss returns nil, pos unchanged ---
# scan(...) is anchored at the current pos. The string still
# contains "world" but not at pos=5 (a space comes first), so
# /world/ misses here.
puts s.scan(/world/).inspect                    # nil
puts s.pos                                      # 5 (unchanged)

# --- whitespace + remainder ---
puts s.scan(/\s+/).inspect                      # " "
puts s.scan(/world/).inspect                    # "world"
puts s.eos?                                     # true
puts s.rest                                     # "" (exhausted)

# --- scan with capture groups ---
# `[]` returns the n-th group of the LAST successful scan;
# [0] is the whole match, [N] is the N-th capture group.
sc = StringScanner.new("AB:CD:EF")
sc.scan(/(\w+):(\w+)/)
puts sc[0]                                      # AB:CD
puts sc[1]                                      # AB
puts sc[2]                                      # CD

# --- a failed scan resets `[]` to nil ---
# After scan(...) returns nil, the previous match data is
# wiped (matching CRuby's behavior).
fc = StringScanner.new("hello")
fc.scan(/hello/)
puts fc[0]                                      # hello
fc.scan(/xyz/)
puts fc[0].inspect                              # nil

# --- ERB-shape probe ---
# The exact regex shape ERB uses in SimpleScanner (a stag
# regex with two groups — content + opener). Pin that the
# scanner walks through correctly and exposes both groups
# via [1] and [2].
src = "Hello <%= name %>!"
stag_re = /(.*?)(<%[%=#]?|\z)/m
scanner = StringScanner.new(src)
out = []
until scanner.eos?
  scanner.scan(stag_re)
  out << scanner[1]
  out << scanner[2]
end
puts out.inspect
# ["Hello ", "<%=", " name %>!", ""]

# --- peek does NOT advance pos ---
pk = StringScanner.new("abcdef")
puts pk.peek(3)                                 # abc
puts pk.pos                                     # 0 (unchanged)
puts pk.scan(/ab/).inspect                      # "ab"
puts pk.peek(2)                                 # cd
puts pk.pos                                     # 2

# --- empty input ---
e = StringScanner.new("")
puts e.eos?                                     # true
puts e.scan(/x/).inspect                        # nil
puts e.scan(/\z/).inspect                       # "" (zero-width hit)
