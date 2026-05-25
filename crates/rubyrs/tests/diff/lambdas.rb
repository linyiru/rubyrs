# `->(params) { body }` — lambda literal. Compiles to a
# Value::Block (we don't model the strict-arity distinction
# between Lambda and Proc; documented in SUBSET.md). Invoke via
# `.call(args)`.

# Basic two-arg lambda.
add = ->(x, y) { x + y }
puts add.call(3, 4)
puts add.call(10, 20)

# One-arg.
sq = ->(x) { x * x }
puts sq.call(5)
puts sq.call(7)

# Zero-arg.
greet = -> { "hello" }
puts greet.call

# Closure: captures outer locals.
def make_multiplier(k)
  ->(x) { x * k }
end

times3 = make_multiplier(3)
puts times3.call(7)
puts times3.call(10)
times10 = make_multiplier(10)
puts times10.call(7)

# Closures over mutable outer state.
n = 5
incr = -> { n += 1 }
incr.call
incr.call
puts n

# Lambda returned from a method, used immediately.
greeter = make_multiplier(2)
puts greeter.call(15)

# Lambda inside a class method.
class Calculator
  def initialize(base)
    @base = base
  end
  def adder
    ->(x) { @base + x }
  end
end

c = Calculator.new(100)
plus = c.adder
puts plus.call(5)
puts plus.call(50)

# Pass a lambda to a method that calls it.
def apply(fn, x)
  fn.call(x)
end

square = ->(x) { x * x }
puts apply(square, 9)

# Returning multiple lambdas from one method.
def make_ops(k)
  add = ->(x) { x + k }
  sub = ->(x) { x - k }
  [add, sub]
end

a, s = make_ops(10)
puts a.call(5)
puts s.call(5)

# Lambdas with body that uses control flow.
classify = ->(n) {
  if n.even?
    "even"
  else
    "odd"
  end
}
puts classify.call(4)
puts classify.call(7)

# Stored in an Array of lambdas.
ops = [
  ->(x) { x + 1 },
  ->(x) { x * 2 },
  ->(x) { x - 3 },
]
out = ops.map { |op| op.call(10) }
p out

# Lambdas as Hash values.
table = {
  square: ->(x) { x * x },
  cube:   ->(x) { x * x * x },
}
puts table[:square].call(4)
puts table[:cube].call(3)
