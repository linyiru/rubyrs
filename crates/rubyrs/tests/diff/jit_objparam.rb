# ADR 0034 Step 1 (param-receiver) — a 1-arg method whose PARAM is an Object, calling
# methods on that param (`def weigh(node); node.value * 2; end`). The param binds as a
# receiver pointer; the body's calls lower via the obj-call PIC. Parity must hold
# interpreter == JIT == CRuby, including the deopts. (This is the foundation for native
# param-receiver recursion; on its own it fires but is dispatch-bound, not yet a YJIT
# win — the recursion win needs the native cross-call.)

class Node
  def initialize(v); @v = v; end
  def value; @v; end             # 0-arg getter
  def scaled(k); @v * k; end      # 1-arg method
end

def weigh(node); node.value * 2 + 1; end       # 0-arg obj-call on the param
def scale(node); node.scaled(3) - node.value; end   # 1-arg + 0-arg obj-calls on the param

n = Node.new(20)
p weigh(n)
p scale(n)

# Driven in a loop (the interpreted-driver case).
s = 0
i = 0
while i < 50
  s += weigh(n)
  i += 1
end
p s

# Polymorphic receiver class via the param: a subclass overriding the called method
# must deopt to the interpreter and stay correct.
class Node2 < Node
  def value; @v + 100; end
end
p weigh(Node2.new(20))
p weigh(n)                       # back to the base class

# A non-Object arg to the same method name must NOT use the obj-param native code
# (it routes to the Int path or interpreter). `weigh` with an Int arg: Int has no
# `value`, so CRuby raises NoMethodError — must match.
begin
  weigh(42)
  p :no_raise
rescue NoMethodError
  p :nomethod
end
