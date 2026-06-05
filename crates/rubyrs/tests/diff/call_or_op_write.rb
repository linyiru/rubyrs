# `recv.attr ||= val` / `&&=` / `+=` (CallOrWriteNode,
# CallAndWriteNode, CallOperatorWriteNode in Prism's AST) —
# attribute-or-write / and-write / op-write sugar. Pre-fix,
# all three tripped the "unsupported node" trap at AST
# lowering. Mirrors the IndexOr/And/OperatorWriteNode arms
# the spike landed earlier (`recv[i] ||= val` etc.).
#
# P3 Sinatra-spike gap: mustermann's `mustermann/ast/node.rb`
# uses `self.payload ||= []` at multiple sites; the spike
# blocked at line 4's `require 'mustermann/ast/node'`.

# 1. `recv.attr ||= val` — falsy receiver replaced, truthy
# preserved. CRuby evaluates the receiver twice (once for
# read, once for write); the rubyrs translator mirrors that
# shape.
class P1
  attr_accessor :v
end
p = P1.new
p.v ||= 99
puts p.v       # nil → 99
p.v ||= 1
puts p.v       # 99 stays
p.v = false
p.v ||= "fallback"
puts p.v       # false → "fallback"

# 2. Same shape with explicit `self.` receiver inside a
# method body — Mustermann's exact pattern.
class P2
  attr_accessor :payload
  def parse_init
    self.payload ||= []
  end
end
o = P2.new
o.parse_init
puts o.payload.inspect    # []
o.payload = [1, 2]
o.parse_init
puts o.payload.inspect    # [1, 2] (truthy, unchanged)

# 3. `recv.attr &&= val` — truthy receiver replaced, falsy
# preserved.
class P3
  attr_accessor :v
end
p = P3.new
p.v &&= 42
puts p.v.inspect          # nil &&= → nil
p.v = "set"
p.v &&= 42
puts p.v                  # "set" truthy → 42

# 4. `recv.attr += val` — binary operator write. Reads
# current value, applies op, writes back via the writer
# method.
class P4
  attr_accessor :c
  def initialize; @c = 0; end
  def bump; self.c += 1; end
end
b = P4.new
3.times { b.bump }
puts b.c                  # 0 + 1 + 1 + 1 = 3

# 5. `+=` with non-self receiver — a local variable.
class P5
  attr_accessor :n
  def initialize; @n = 10; end
end
q = P5.new
q.n += 5
puts q.n                  # 15
q.n *= 2
puts q.n                  # 30

# 6. Mixed with chained writers — verify the read/write
# receiver split holds across nesting.
class Outer
  attr_accessor :inner
end
class Inner
  attr_accessor :val
end
ow = Outer.new
ow.inner = Inner.new
ow.inner.val ||= "init"
puts ow.inner.val
ow.inner.val ||= "ignored"
puts ow.inner.val
