# The interpreter's hot-op front (`step`) handles only the common case of
# each op and hands the rest to the full match. These pin the hand-offs.
# Only a method that creates no block gets arena locals (the hot path), so
# every method here except `captured` must stay block-free.

# BinOpInt / BinOpLocalLocal: overflow promotes, zero divisors raise,
# Float operands dispatch.
def arith(a, b)
  [a + 1, a - 1, a * 2, a + b, a - b, a * b, a < b, a == b, a <=> b]
end
p arith(1, 2)
p arith(2**62, 2**62)
p arith(-(2**62), 2**62)
p arith(1.5, 2)
def divs(a, b)
  r = []
  begin; r << a / 0; rescue ZeroDivisionError => e; r << e.message; end
  begin; r << a % 0; rescue ZeroDivisionError => e; r << e.message; end
  begin; r << a / b; rescue ZeroDivisionError => e; r << e.message; end
  begin; r << a % b; rescue ZeroDivisionError => e; r << e.message; end
  r
end
p divs(7, 0)
p divs(7, 2)

# IncLocalNoPush: Int in place, a Float through `+`.
def incs(x)
  i = x
  i += 1
  i += 1
  i
end
p incs(1)
p incs(1.5)

# Locals captured by a block leave the arena path.
def captured
  a = 1
  b = 2
  f = -> { a += b; b += 1 }
  f.call
  f.call
  s = a + b
  a += 1
  [a, b, s]
end
p captured

# Truthiness for the conditional jump.
def truthy(v) = v ? :t : :f
p [nil, false, 0, "", [], true].map { |v| truthy(v) }

# Calls with and without an explicit receiver, including a trailing
# brace Hash, which stays positional.
class Box
  def one(h) = h
  def zero = :zero
  def run(o)
    x = o
    [x.zero, one({a: 1}), zero, o.one({b: 2})]
  end
end
p Box.new.run(Box.new)
