# Regexp#named_captures — name → [1-based capture indices]. All-named
# patterns match CRuby exactly (mixed named/unnamed differ by engine
# numbering — out of scope; Sinatra/mustermann routes are all-named).
p(/(?<a>.)(?<b>.)/.named_captures)
p(/(?<year>\d+)-(?<mon>\d+)/.named_captures)
p(/no groups here/.named_captures)
p(/(?<only>\w+)/.named_captures)
p(/(?<h>\d+):(?<m>\d+):(?<s>\d+)/.named_captures)
p(/(?<a>.)(?<b>.)/.named_captures.class)
