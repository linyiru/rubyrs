# A LARGE (>256 char) lookaround/possessive pattern whose fancy-regex
# build is DEFERRED to first use must still construct and match exactly
# as an eagerly-built one. Locks in the lazy-fancy compilation path.
unit = '(?:[a-z]++(?=\d)\d++)'
pat  = '\A' + ([unit] * 18).join('[-_]?') + '\z'
re   = Regexp.new(pat)
puts pat.length > 256          # confirm it crosses the lazy threshold
puts re.is_a?(Regexp)

good = (["a1"] * 18).join      # each unit: letters then digits
puts re.match?(good)           # should match
puts re.match?("a1b2-c3")      # too few units -> no match
puts re.match?("")             # no match

# lookahead semantics preserved: digit must follow the letters
puts(/foo(?=bar)/ =~ "foobar").inspect
puts(/foo(?=bar)/ =~ "foobaz").inspect

# the same large pattern reused (cached engine) still matches
puts re.match?(good)
