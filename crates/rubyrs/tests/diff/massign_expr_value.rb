# The VALUE of a multiple-assignment is the original RHS, not the
# coerced destructuring Array: `(a, b = nil)` => nil, `(a, b = 42)`
# => 42, `(a, b = [1,2])` => [1,2]. Critically `while (x, y =
# queue.shift)` must terminate when shift returns nil (the value is
# nil, falsy) — it looped forever before. (zeitwerk's eager-load
# directory queue.)

p((a, b = nil))
p((c, d = [1, 2]))
p((e, f = 42))
p((g, h = "str"))
p((i, j, *k = [1, 2, 3, 4, 5]))
p((l, *m, n = 9))
p((*o, p1 = [1, 2, 3]))

# assignment still happens correctly
a, b = nil
p [a, b]                       # [nil, nil]
c, d = [10, 20]
p [c, d]
e, f, *g = [1, 2, 3, 4]
p [e, f, g]
*h, i = [1, 2, 3]
p [h, i]

# the loop idiom terminates
queue = [[1, :a], [2, :b], [3, :c]]
seen = []
while (cur, sym = queue.shift)
  seen << [cur, sym]
end
p seen

# splat target value
r = (s, *t = 1, 2, 3)
p r                            # [1, 2, 3]
p [s, t]

# massign with method-call (index) targets keeps RHS as value
arr = [0, 0]
v = (arr[0], arr[1] = 8, 9)
p v                            # [8, 9]
p arr
