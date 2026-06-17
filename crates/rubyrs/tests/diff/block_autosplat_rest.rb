# CRuby auto-splats a single Array arg into a block that expects more
# than one value — including a block with FIXED params + a rest param
# (`|a, *b|`, `|x, y, z, *r|`). A rest-ONLY block (`|*a|`) captures the
# array as-is. rubyrs skipped the splat for any rest block, so rss's
# `[[name, occurs, type, *args], …].each { |name, occurs, type, *args| … }`
# bound `name` to the whole row.
[[1, 2, 3]].each { |a, *b| p [a, b] }                 # [1, [2, 3]]
[["pubDate", "?", :date, :rfc822]].each { |n, o, t, *a| p [n, o, t, a] }  # ["pubDate", "?", :date, [:rfc822]]
[[1, 2]].each { |a, b| p [a, b] }                     # [1, 2]   (already worked)
[[1, 2]].each { |*a| p a }                            # [[1, 2]] (rest-only: no splat)
[[1, 2]].each { |a| p a }                             # [1, 2]   (single param: no splat, binds whole)
[[1]].each { |a, *b| p [a, b] }                       # [1, []]
[[1, 2, 3, 4]].each { |a, b, *c| p [a, b, c] }        # [1, 2, [3, 4]]

# map with rest destructuring
r = [[10, 20, 30]].map { |head, *tail| [head, tail] }
p r                                                   # [[10, [20, 30]]]

# a non-array single arg is NOT splat (binds first param, rest empty)
[5].each { |a, *b| p [a, b] }                         # [5, []]
