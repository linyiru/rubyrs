# ADR 0034 Step 1 ("1a") — a compiled caller cross-calls an obj-param method with an
# OBJECT arg native->native (`weigh(@h)` inside a hot loop). The ivar/local is loaded as
# a receiver pointer and passed to the callee's obj-param variant. Parity must hold
# interpreter == JIT == CRuby, including the deopts.

class Helper
  def initialize(k); @k = k; end
  def value; @k; end
end

def weigh(node); node.value * 2 + 1; end       # obj-param leaf (non-self callee)

class Driver
  def initialize(h); @h = h; end
  def run(n)
    s = 0
    i = 0
    while i < n
      s += weigh(@h)                             # non-self Object-arg cross-call
      i += 1
    end
    s
  end
end

p Driver.new(Helper.new(20)).run(50)
p Driver.new(Helper.new(0)).run(50)
p Driver.new(Helper.new(20)).run(0)             # empty loop

# Polymorphic receiver class for the arg: a subclass overriding `value` must deopt
# (the obj-call PIC inside `weigh` class-guards) and stay correct.
class Helper2 < Helper
  def value; @k + 1000; end
end
p Driver.new(Helper2.new(20)).run(50)

# The arg ivar is not an Object (Int) — `weigh(Int)` raises NoMethodError on `.value`;
# must match CRuby (guard the raise; only stdout is compared).
class Bad
  def initialize; @h = 7; end
  def run; weigh(@h); end
end
begin
  Bad.new.run
  p :no_raise
rescue NoMethodError
  p :nomethod
end
