# A `return` inside a LAMBDA is local — it returns from the lambda and
# execution continues at the caller. (A `return` in an ordinary proc or
# block still returns from the lexically-enclosing method.)

# Direct return in a lambda body.
f = ->(x) { return x * 2 }
p f.call(5)

# The caller continues after the lambda returns.
def via_lambda
  g = -> { return 10 }
  r = g.call
  r + 1
end
p via_lambda

# Early return inside a lambda.
h = ->(x) { return :neg if x < 0; :pos }
p h.call(-1)
p h.call(3)

# A `return` in a block NESTED inside a lambda exits the LAMBDA.
k = ->(xs) { xs.each { |x| return x if x > 2 }; :none }
p k.call([1, 2, 3, 4])
p k.call([1, 2])

# Nested lambdas: the inner return is local to the inner lambda.
outer = lambda do
  inner = -> { return :inner }
  v = inner.call
  [:outer, v]
end
p outer.call

# `lambda { }` (Kernel form) also returns locally.
p lambda { return 42 }.call

# Contrast: a proc's `return` returns from the enclosing METHOD.
def via_proc
  pr = proc { return 99 }
  pr.call
  :unreached
end
p via_proc

# And a plain block's `return` (each) returns from the method too.
def via_each
  [1, 2, 3].each { |x| return x * 100 if x == 2 }
  :unreached
end
p via_each

# Lambda return value flows into the surrounding expression.
m = ->(a, b) { return a if a > b; b }
p [m.call(5, 3), m.call(2, 7)]
