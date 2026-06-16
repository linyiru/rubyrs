# Regexp#names — named capture-group names in group-index order.
p(/(?<one>.)(?<two>.)(?<three>.)/.names)   # ["one", "two", "three"]
p(/(?<z>.)(?<a>.)(?<m>.)/.names)           # declaration order, not sorted
p(/(\d)(?<x>\w)/.names)                     # mixed unnamed + named -> ["x"]
p(/nope/.names)                             # []
p(Regexp.new("(?<y>.)").names)             # ["y"]
p(/abc/.names.any?)                         # false  (mustermann's branch)
p(/(?<k>.)/.names.any?)                     # true
