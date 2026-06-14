# Regexp.union must combine each Regexp member by its #to_s form
# `(?on-off:source)` — NOT the bare source. A member's own flags
# (esp. /x extended mode, where literal whitespace in the source is
# insignificant; and /i) have to stay scoped to that member, or the
# union silently changes that member's meaning. This is exactly how
# rack's request.rb builds its ipv6 + trusted_proxies matchers.

# /x member: the spaces around `::` are insignificant under /x, so the
# union must NOT treat them as literal spaces.
ipv6 = Regexp.union(
  /(?:[0-9A-Fa-f]{1,4}:){7}[0-9A-Fa-f]{1,4}/x,
  /(?:[0-9A-Fa-f]{1,4}(?::[0-9A-Fa-f]{1,4})*)? :: (?:[0-9A-Fa-f]{1,4}(?::[0-9A-Fa-f]{1,4})*)?/x,
)
p !ipv6.match("2001:db8:cafe::17").nil?   # true
p !ipv6.match("fd00::").nil?              # true

# /i member: case-insensitivity must survive the union.
trusted = Regexp.union(
  /\A::1\z/,
  /\Af[cd][0-9a-f]{2}(?::[0-9a-f]{0,4}){0,7}\z/i,
  /\Alocalhost\z|\Aunix(\z|:)/i,
)
p !trusted.match("FD00::").nil?           # true (uppercase)
p !trusted.match("fd00::").nil?           # true
p !trusted.match("LOCALHOST").nil?        # true
p trusted.match("example.org").nil?       # true (no match)

# String members are still escaped + a bare flagless Regexp keeps working.
u = Regexp.union("a.b", /c+/)
p !u.match("a.b").nil?                     # true (dot escaped)
p u.match("axb").nil?                      # true (dot is literal)
p !u.match("ccc").nil?                     # true

# to_s of a union member is reflected in the combined source.
p Regexp.union(/x/i, /y/m).source
