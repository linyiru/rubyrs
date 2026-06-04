# MatchData full surface — `String#match` returns a MatchData
# instance with `[0]` / `[N]` indexed access, captures, to_a,
# size/length, to_s, inspect, AND the regex-bound context
# methods (`pre_match`, `post_match`, `string`, `regexp`).
# Pre-upgrade, String#match (String arg) returned the matched
# substring as a plain String, breaking any call site that
# read `.captures` / `.pre_match` on it.

# Regex arg with multiple capture groups.
m = "hello world!".match(/(\w+) (\w+)/)
p m.class
p m[0]
p m[1]
p m[2]
p m.captures
p m.to_a
p m.size
p m.length
p m.to_s
p m.pre_match
p m.post_match
p m.string
p m.regexp.class

# inspect — `#<MatchData "<whole>" 1:"<cap>" 2:"<cap>" ...>` shape.
puts m.inspect

# No capture groups — `inspect` omits the per-group list and
# captures is empty.
m2 = "abcXYZdef".match(/[A-Z]+/)
puts m2.inspect
p m2.captures
p m2[0]
p m2.pre_match
p m2.post_match

# String arg coerces to Regex — `"text/html".match("text")`
# behaves the same as `match(/text/)` after the upgrade.
m3 = "text/html".match("text")
p m3.class
p m3[0]
p m3.pre_match
p m3.post_match

# String arg as a real pattern (metachars active).
m4 = "abc123def".match("\\d+")
p m4[0]

# Failed match returns nil (NOT MatchData).
p "abc".match(/xyz/)
p "abc".match("xyz")

# Non-participating capture group (alternation arm didn't match)
# serialises as nil in @caps, inspect renders `N:nil`.
m5 = "abc".match(/(\d+)|(\w+)/)
p m5.captures
puts m5.inspect

# MatchData equality / inspect parity across both match shapes
# (regex literal vs. coerced String) — same captured content
# means same #to_a; the only divergence is `.regexp` (a Regexp
# either way, but a fresh compile for the String coercion).
str_match = "axb".match("a(.)b")
re_match  = "axb".match(/a(.)b/)
p str_match.to_a == re_match.to_a
p str_match.captures == re_match.captures

# Round-tripping pre_match + match + post_match reconstructs
# the original string.
str = "before-MID-after"
m6 = str.match(/MID/)
p (m6.pre_match + m6.to_s + m6.post_match) == str
