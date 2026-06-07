# An anonymous Struct class held ONLY inside a container (an Array or
# Hash constant, an instance ivar) — never bound directly to a constant
# — must keep its class-level @__struct_attrs alive across GC. The
# constant/global root scan reaches the container but the swept element
# is the Class; without a Value::Class arm in visit_value its ivars were
# never marked and got freed mid-use (use-after-free under STRESS_GC).
PLUGINS = [Struct.new(:a, :b, :c)].freeze       # class inside an Array
REG = { widget: Struct.new(:x, :y) }.freeze     # class inside a Hash

class Registry
  def initialize
    @kind = Struct.new(:n)                       # class inside an ivar
  end
  attr_reader :kind
end
reg = Registry.new

out = []
300.times do |i|
  s = PLUGINS[0].new(i, i * 2, i * 3)
  w = REG[:widget].new(i, i + 1)
  k = reg.kind.new(i)
  out << s.a + s.b + s.c + w.x + w.y + k.n
end
p out.first(3)
p out.last(3)
p out.sum
p PLUGINS[0].members
p REG[:widget].members
p reg.kind.members
