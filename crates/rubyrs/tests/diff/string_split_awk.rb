# Regression: `String#split(" ")` (literal single-space sep) is CRuby's
# AWK-style special case — collapses runs of ANY whitespace (`\t \n` …),
# strips leading empties, and interacts with `limit` in a documented but
# easily-missed way. Without the special case the rubyrs default for
# every other 1-char sep applies (literal " " match, leading/trailing
# empties become their own fields), diverging from CRuby on every
# space-tokenised input — query-string parsing, shell-style arg parsing,
# etc. CRuby is the oracle.

# 1. Bare " " sep — strips leading + trailing, collapses runs.
puts "  a  b  c  ".split(" ").inspect       # ["a", "b", "c"]
puts "a\t b\nc".split(" ").inspect           # ["a", "b", "c"]
puts " ".split(" ").inspect                  # []
puts "".split(" ").inspect                   # []

# 2. Positive limit: skip leading WS, then take limit-1 WS-delimited
# tokens; last field = unsplit remainder (including any trailing WS).
puts "  a  b  c  ".split(" ", 2).inspect     # ["a", "b  c  "]
puts "a b c d".split(" ", 3).inspect         # ["a", "b", "c d"]
puts "a b c d".split(" ", 100).inspect       # ["a", "b", "c", "d"]
puts "a b".split(" ", 1).inspect             # ["a b"]

# 3. Negative limit: keep one trailing "" if source ended in WS.
puts "  a  b  c  ".split(" ", -1).inspect    # ["a", "b", "c", ""]
puts "\n\na\nb\n\n".split(" ", -1).inspect   # ["a", "b", ""]
puts "a b".split(" ", -1).inspect            # ["a", "b"]

# 4. Whitespace-only source quirk:
#   limit == 0 → drops trailing empties → []
#   limit != 0 → [""] (the single leading-stripped token survives)
puts "    ".split(" ").inspect               # []
puts "    ".split(" ", -1).inspect           # [""]
puts "    ".split(" ", 3).inspect            # [""]

# 5. No-arg `split` is the same shape as `split(" ")`.
puts "  a  b  c  ".split.inspect             # ["a", "b", "c"]

# 6. Non-space sep is unaffected — literal " " is the only special case.
puts "a,b,c".split(",").inspect              # ["a", "b", "c"]
puts "a, b, c".split(", ").inspect           # ["a", "b", "c"]
