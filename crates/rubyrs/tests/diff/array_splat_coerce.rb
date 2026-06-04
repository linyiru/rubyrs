# `[*x]` array-literal splat coerces non-Array values via the
# CRuby `Array(x)` contract:
#   - Array → unchanged
#   - nil   → []
#   - other → [other]   (`[*"foo"]` → `["foo"]`)
# Surfaced by sinatra-contrib/MultiRoute's `routes = [*args.pop]`
# pattern — when `args.pop` returns a String the routes loop
# tripped `String#each` pre-fix.

# Singleton splat.
p [*nil]
p [*"hello"]
p [*42]
p [*[1, 2, 3]]
p [*:sym]

# Splat mixed with other elements.
p [1, *"two", 3]
p [*"a", "b", *"c"]

# Splat of nil mid-list still collapses to nothing.
p [1, *nil, 2]
