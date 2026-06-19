# MatchData#names — array of named-capture group names in pattern
# order; empty when the pattern had no named groups. Mirrors
# Regexp#names. Used by mustermann's regexp pattern matching.

m = "foobar".match(/(?<a>foo)(?<b>bar)/)
p m.names
p m.named_captures

# no named groups → empty array
p "x".match(/(x)/).names

# non-participating named group still listed (nil capture)
m2 = "foo".match(/(?<a>foo)|(?<b>bar)/)
p m2.names
p m2[:a]
p m2[:b]
