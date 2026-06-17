# A trailing comma in a block parameter list (`|name,|`) is an
# implicit rest: it makes the block multi-arg, so a single yielded
# Array auto-splats and `name` binds element 0 (the rest discarded).
# rss's iTunes parser relies on `[["name"],["email"]].each { |n,| … }`.

# Single required + trailing comma: takes first element of each Array.
out = []
[["name"], ["email"]].each { |n,| out << n }
p out

# Numeric arrays.
firsts = []
[[1, 2, 3], [4, 5, 6]].each { |a,| firsts << a }
p firsts

# Two requireds + trailing comma: first two elements, rest dropped.
pairs = []
[[10, 20, 30, 40]].each { |a, b,| pairs << [a, b] }
p pairs

# Non-Array element with trailing comma: value binds directly.
singles = []
[7, 8].each { |x,| singles << x }
p singles

# Interpolating the bound value (the exact rss failure shape).
names = [["author"], ["duration"]]
names.each { |n,| puts "itunes_#{n}" }
