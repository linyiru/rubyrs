# Native iterator drivers re-use one block frame across elements
# (issue #382). Every control-flow exit out of the block body must
# behave exactly as with a fresh frame per element.

a = [1, 2, 3, 4, 5]

# Block-local variables start nil on every iteration.
a.each { |x| p [x, defined?(y) ? y : :undef]; y = x * 10 }
a.each { |x| z ||= x; p z }

# next / next value / break / break value.
r = a.each { |x| next if x.even?; p x }
p r.equal?(a)
p(a.each { |x| break x * 100 if x == 3 })
p(a.map { |x| next x * 2 if x.odd?; x })
p(a.each_with_index { |x, i| break [x, i] if i == 2 })

# redo re-runs the body in place (params unchanged).
tries = 0
a.each { |x| tries += 1; redo if x == 2 && tries < 4; p [x, tries] }

# return from the enclosing method, directly and through nesting.
def first_even(xs) = xs.each { |x| return x if x.even? } && :none
p first_even([1, 3, 4, 5])
p first_even([1, 3])
def nested_ret(xs)
  xs.each { |x| xs.each { |y| return [x, y] if x + y == 7 } }
  :none
end
p nested_ret(a)

# ensure inside the block runs on every exit kind.
log = []
a.each { |x| begin; next if x == 1; break if x == 3; log << x; ensure; log << -x; end }
p log
def ret_through_ensure(xs, log)
  xs.each { |x| begin; return x if x == 2; ensure; log << [:e, x]; end }
end
log = []
p ret_through_ensure(a, log)
p log
log = []
p(a.each { |x| begin; break :b if x == 2; ensure; log << x; end })
p log

# rescue inside the block, and an exception escaping it.
p(a.map { |x| begin; raise "e#{x}" if x.odd?; x; rescue => e; e.message; end })
begin
  a.each { |x| raise ArgumentError, "at #{x}" if x == 4; p x }
rescue ArgumentError => e
  p e.message
end
# retry inside the block body.
n = 0
a.each { |x| begin; n += 1; raise "r" if x == 2 && n < 4; p [x, n]; rescue; retry; end }

# while loops (with break / next) inside the block body.
a.each { |x| i = 0; while i < x; i += 1; next if i == 1; break if i == 3; end; p [x, i] }

# throw / catch across the iterator.
p(catch(:done) { a.each { |x| throw :done, x if x == 4 } })

# Closures created in the block capture that iteration's binding.
procs = []
a.each { |x| procs << -> { x } }
p procs.map(&:call)

# Nested iteration over the same array and re-entrant use of one block.
p(a.each_with_object([]) { |x, acc| a.each { |y| acc << [x, y] if x == y } })
blk = proc { |x| x > 2 ? x : [x, [x + 1].map(&blk)] }
p a.map(&blk)

# The walk re-reads the array: appends are visited, shrinks stop it.
g = [1, 2]
g.each { |x| g << x + 10 if g.size < 5; p x }
h = [1, 2, 3, 4]
h.each { |x| h.pop; p x }

# Outer locals written by the block are shared, not snapshotted.
sum = 0
a.each { |x| sum += x }
p sum
def acc(xs)
  total = 0
  xs.each { |x| xs.each { |y| total += x * y } }
  total
end
p acc(a)

# Other drivers on the same path.
p(a.select { |x| x.odd? }, a.reject { |x| x.odd? }, a.find { |x| x > 2 })
p(a.any? { |x| x > 4 }, a.all? { |x| x > 0 }, a.none? { |x| x > 9 })
t = []
3.times { |i| t << i }
(1..4).each { |i| t << i }
{ a: 1, b: 2 }.each { |k, v| t << [k, v] }
{ a: 1, b: 2 }.each { |kv| t << kv }
p t
p(5.times { |i| break i if i == 3 })
p((1..10).each { |i| break i * 2 if i == 4 })
p({ a: 1, b: 2 }.each { |k, v| break k if v == 2 })

# Lambdas and method objects as the block.
lam = ->(x) { x * 3 }
p a.map(&lam)
p a.map(&method(:Integer))

# Optional / destructured / numbered params re-bound per element.
[10, 20].each_with_index { |a, b = 5| p [a, b] }
[10, 20].each_with_index { |a, b = 5, c = 6| p [a, b, c] }
{ a: 1, b: 2 }.each { |k, v = 9| p [k, v] }
[1, 2].each { |x, y = 3| p [x, y] }
[[1, 2], [3, 4]].each { |x, y = 3| p [x, y] }
3.times { |i, j = 7| p [i, j] }
[1, 2].each { |x = 4| p x }
[[1, [2, 3]], [4, [5, 6]]].each { |a, (b, c)| p [a, b, c] }
[[1, 2], [3, 4]].each_with_index { |(a, b), i| p [a, b, i] }
{ a: [1, 2] }.each { |k, (x, y)| p [k, x, y] }
[1, 2, 3].each { |x; y| p [x, y]; y = x }
[1, 2].each { |x, **kw| p [x, kw] }
[1, 2].each { |*r| p r }
[1, 2].each { |x, *r| p [x, r] }
[1, 2].each { p it }
[1, 2].each { p _1 }
{ a: 1 }.each { p [_1, _2] }
