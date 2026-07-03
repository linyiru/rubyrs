# String#[](substr) / #slice(substr) — the substring-search form:
# returns a new String equal to substr when the receiver contains it,
# else nil. Surfaced by rubocop 1.88's Style/MagicCommentFormat
# (`text[wrong_separator]` on every magic comment), where the missing
# form crashed the cop and silently blocked every result-cache save.
s = "frozen_string_literal: true"
p s["_"]
p s["-"]
p s["string_literal"]
p s["absent"]
p s.slice(": ")
p s.slice("nope")

# Result is a new object, not the argument itself
needle = "frozen"
got = s[needle]
p got
p got.equal?(needle)

# Empty substring always matches (CRuby returns "")
p s[""]
p ""[""]

# Empty receiver contains nothing else
p ""["x"]

# Multibyte
u = "héllo wörld"
p u["wörld"]
p u["ö"]
p u["x"]

# Case-sensitive
p s["FROZEN"]
