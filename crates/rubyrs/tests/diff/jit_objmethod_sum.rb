# B4 (ADR 0034): `objs.sum { |o| o.method(CONST) }` over a monomorphic Object
# array compiles to a whole-loop native driver with a direct native->native method
# call (no per-element do_call) + class-guard deopt. Parity must hold
# interpreter == JIT == CRuby on every shape, including the polymorphic deopt.

class Acct
  def initialize(b); @b = b; end
  def fee(r); @b * 3 + r - 1; end
end

accts = (0...12).map { |i| Acct.new(i * 100) }
p accts.sum { |o| o.fee(7) }
p accts.sum { |o| o.fee(0) }
p accts.sum(1000) { |o| o.fee(7) }      # explicit init seed
p [].sum { |o| o.fee(7) }               # empty -> 0

# Bignum overflow inside the accumulation must deopt and still be exact.
big = [Acct.new(4_000_000_000_000_000_000)]
p big.sum { |o| o.fee(0) }              # 3 * 4e18 overflows i64 -> bignum

# Polymorphic array: a second class with a different `fee` must deopt to the
# generic loop and remain correct (not silently use the first class's code).
class Acct2
  def initialize(b); @b = b; end
  def fee(r); @b * 2 + r; end
end
mixed = [Acct.new(10), Acct2.new(10), Acct.new(20)]
p mixed.sum { |o| o.fee(1) }

# A non-Object element (Int) in the array must deopt cleanly.
class Acct3
  def initialize(b); @b = b; end
  def fee(r); @b + r; end
end
# uniform class but the method reads an ivar that is sometimes non-Int -> deopt.
a = Acct3.new(5)
b = Acct3.new(2)
p [a, b].sum { |o| o.fee(100) }
