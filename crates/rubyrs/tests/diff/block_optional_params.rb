# Optional POSITIONAL parameters in blocks and lambdas (`|a, b = 1|`,
# `->(a, b = 1)`). Previously dropped entirely (`->(a, b=10){}.call(1)`
# → [1, nil], and even .call(1, 2) lost the 2). Now they take a real
# positional slot and apply the default when no arg is supplied.

# Lambda.
f = ->(a, b = 10) { [a, b] }
p f.call(1)
p f.call(1, 2)

# Proc.
g = proc { |a, b = 5| [a, b] }
p g.call(1)
p g.call(1, 2)

# Multiple optionals.
h = ->(a, b = 2, c = 3) { [a, b, c] }
p h.call(1)
p h.call(1, 20)
p h.call(1, 20, 30)

# Optional + rest.
k = ->(a, b = 9, *rest) { [a, b, rest] }
p k.call(1)
p k.call(1, 2)
p k.call(1, 2, 3, 4)

# Default expression may reference an earlier parameter.
m = ->(a, b = a * 10) { [a, b] }
p m.call(3)
p m.call(3, 7)

# Block forms: each / yield / auto-splat of a single Array.
r = []
[[1, 2], [3]].each { |a, b = 99| r << [a, b] }
p r
def takes; yield 1; end
takes { |a, b = 8| p [a, b] }
[[1, 2]].each { |a, b = 0| p [a, b] }
