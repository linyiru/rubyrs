# Implicit block/lambda params: numbered `_1`/`_2` and the Ruby 3.4
# `it`. Prism gives these as NumberedParametersNode / ItParametersNode
# (no named param list) rather than BlockParametersNode; the translator
# synthesizes `_1`..`_N` / `it` slots so the yielded args bind. Without
# it they read back as nil.

# _1 single
[10, 20, 30].each { p _1 }

# _1 + _2 auto-splat over pairs
{ a: 1, b: 2 }.each { p [_1, _2] }
[10, 20].each_with_index { p [_1, _2] }

# _1 + _2 in a transform, then _1 again
p [[1, 2], [3, 4]].map { _1 + _2 }

# it param
p [5, 6, 7].map { it * 2 }
p [1, 2, 3].select { it.odd? }

# nested blocks: inner _1 is the inner block's arg
p [[1, 2], [3, 4]].map { |row| row.map { _1 * 10 } }

# bare `it` is the param, but `it(...)` / `it` with a real def is a call
def it(x = 99); "method_it(#{x})"; end
p [1].map { it }
p it
p it(5)

# lambda literals take implicit params too
f = -> { _1 + 1 }
p f.call(10)
g = -> { it * 3 }
p g.call(4)
h = -> { _1 * _2 }
p h.call(6, 7)
