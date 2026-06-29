# ADR 0034 Step 1 — explicit-receiver method calls on another object (`@h.method(arg)`,
# recv != self) inside a compiled body lower to a native->native PIC call. Parity must
# hold interpreter == JIT == CRuby on every shape, including the deopts.

class Helper
  def initialize(k); @k = k; end
  def compute(x); x * 2 + @k; end
end

class Driver
  def initialize(h); @h = h; end
  def run(n)
    s = 0
    i = 0
    while i < n
      s += @h.compute(i % 10)   # explicit-recv 1-arg call on an ivar receiver
      i += 1
    end
    s
  end
end

p Driver.new(Helper.new(7)).run(50)
p Driver.new(Helper.new(0)).run(50)
p Driver.new(Helper.new(7)).run(0)     # empty loop

# Polymorphic receiver class: a subclass overriding `compute` must deopt to the
# interpreter and stay correct (not reuse the first class's native code).
class Helper2 < Helper
  def compute(x); x * 100 + @k; end
end
p Driver.new(Helper2.new(1)).run(50)

# Bignum overflow inside the accumulation must deopt and be exact.
class Big
  def initialize; @b = 4_000_000_000_000_000_000; end
  def compute(x); @b + x; end
end
p Driver.new(Big.new).run(3)

# The receiver ivar is NOT an Object (an Int) — the obj-ptr load must deopt cleanly,
# never dereference a null receiver. (Regression guard: this used to crash to empty
# output because the method codegen continues past a deopt flag.)
class Box
  def initialize(v); @v = v; end
  def veq?(o); @v.eql?(o); end          # @v is an Int, .eql? is a builtin
end
b = Box.new(7)
p b.veq?(7)
p b.veq?(8)

# Implicit-self identity calls (CallNoRecv, not explicit-recv) must still be correct.
U = Object.new
def U.default(x, y = self)
  if equal?(x) then :x
  elsif equal?(y) then :y
  else x end
end
p U.default(U)
p U.default(42)

# --- 0-arg explicit-recv calls `@c.val` (the canonical attribute/derived shape) ---
class Cell
  def initialize(v); @v = v; end
  def val; @v * 3 + 1; end           # 0-arg, real arithmetic
end
class Agg
  def initialize(c); @c = c; end
  def run(n)
    s = 0; i = 0
    while i < n
      s += @c.val                    # 0-arg explicit-recv on an ivar
      i += 1
    end
    s
  end
end
p Agg.new(Cell.new(9)).run(40)
p Agg.new(Cell.new(0)).run(40)
p Agg.new(Cell.new(9)).run(0)        # empty

# 0-arg polymorphic deopt.
class Cell2 < Cell
  def val; @v + 1; end
end
p Agg.new(Cell2.new(5)).run(40)

# A 0-arg method on a 0-arg method must NOT be served at a wrong-arity call (the
# arity-swallow guard): `cell.val(99)` raises ArgumentError, never silently runs.
begin
  Cell.new(1).val(99)
  p :no_raise
rescue ArgumentError
  p :argerr
end
