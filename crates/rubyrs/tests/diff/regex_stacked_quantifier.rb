# Ruby "stacked quantifiers" — a count `{n,m}` itself followed by `*`
# (or another `{…}`). Onigmo accepts these; the Rust engines reject
# `{n,m}*` ("Target of repeat operator is invalid"), so rubyrs rewrites
# `X{n,m}*` → `(?:X{n,m})*`. Driver: RuboCop's Style/StringLiterals
# `double_quotes_required?` regex `/'|(?<!\\)\\{2}*\\(?![\\"])/x`.

# The exact RuboCop regex (look-around + stacked quantifier together).
r = /'|(?<! \\) \\{2}* \\ (?![\\"])/x
p r.match?(%q{"hello"})    # false — no escapes
p r.match?(%q{can't})      # true — has a '
p r.match?('a\nb')         # true — has an escape
p r.match?("plain")        # false

# Stacked quantifier on a literal atom.
p(/a{2}*/.match?("aaaa"))   # true
p(/a{2}*z/.match?("z"))     # true (zero pairs of "a")
p(/^a{2}*$/.match?("aaa"))  # false (odd count)
p(/^a{2}*$/.match?("aaaa")) # true

# Stacked quantifier on an escaped backslash.
p("\\\\\\\\".match?(/\A\\{2}*\z/))  # 4 backslashes — true
p("\\\\\\".match?(/\A\\{2}*\z/))    # 3 backslashes — false

# `{n}?` (lazy) and `{n}+` (possessive) are MODIFIERS, NOT stacked —
# must keep working unchanged.
p("aa".match?(/a{2}?/))     # true (lazy {2})
p "aaa".match(/a{1,3}?/)[0] # "a" (lazy: fewest)
p("aa".match?(/a{2}+/))     # true (possessive {2})

# A `{` inside a char class is literal, not a quantifier.
p("a{2}*".match?(/[{}2*a]+/))  # true
