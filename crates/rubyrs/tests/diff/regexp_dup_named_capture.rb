# Ruby/Oniguruma allows several capture groups to share a name —
# `(?<a>X)|(?<a>Y)` — and `m[:a]` resolves to the arm that
# PARTICIPATED (the last matched group), not the textually-last group
# (nil for an alternation). The linear `regex` crate rejects duplicate
# names so these run on fancy-regex, which keeps every group's value
# but collapses the NAME onto one group; the name->value resolution is
# recovered from the pattern source. This is rack's request.rb
# AUTHORITY matcher.

# alternation, first arm matches
p "foo".match(/(?<v>foo)|(?<v>bar)/)[:v]            # "foo"
# alternation, second arm matches
p "bar".match(/(?<v>foo)|(?<v>bar)/)[:v]            # "bar"
# both groups present, last matched wins
p "XY".match(/(?<n>.)(?<n>.)/)[:n]                  # "Y"
p "XY".match(/(?<n>.)(?<n>.)/).named_captures       # {"n"=>"Y"}
# =~ / $~ path
"k=v" =~ /(?<p>\w)=(?<p>\w)/
p $~[:p]                                            # "v"

# The rack AUTHORITY shape: a bracketed IPv6 OR a bare host, the
# `address` name on BOTH arms; combined via Regexp.union of /x parts;
# the OUTER regex is /x WITH a comment that itself contains parens.
ipv6 = Regexp.union(
  /(?:[0-9A-Fa-f]{1,4}:){7}[0-9A-Fa-f]{1,4}/x,
  /(?:[0-9A-Fa-f]{1,4}(?::[0-9A-Fa-f]{1,4})*)? :: (?:[0-9A-Fa-f]{1,4}(?::[0-9A-Fa-f]{1,4})*)?/x,
)
authority = /
  \A
  (?<host>
    # bracketed IPv6 (parens inside this comment must not miscount)
    \[(?<address>#{ipv6})\]
    |
    (?<address>[[[:graph:]&&[^\[\]]]]*?)
  )
  (:(?<port>\d+))?
  \z
/x
%w([2001:db8:cafe::17]:47011 example.com:80 [fe80::1] 1.2.3.4).each do |a|
  m = authority.match(a)
  p [a, m && m[:host], m && m[:address], m && m[:port]]
end

# single-named groups (the common case) must be byte-for-byte unchanged
p "2024-01-15".match(/(?<y>\d+)-(?<m>\d+)-(?<d>\d+)/).named_captures
p "ab12".match(/(?<a>[a-z]+)(\d+)/)[:a]             # "ab"
