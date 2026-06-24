# Superinstruction LoadLocalCall: a zero-arg method call whose receiver
# is a local variable (`x.foo`) fuses LoadLocal+Call into one op. Must be
# behaviorally identical to the unfused path across return types,
# method_missing, getters, chaining, nil receiver, and private calls.
class Widget
  attr_reader :name
  def initialize(n); @name = n; end
  def shout; @name.upcase; end
  def itself2; self; end
  def method_missing(m, *a); "mm:#{m}"; end
  def respond_to_missing?(*); true; end
end
w = Widget.new("box")
p w.name                 # "box"  (getter — LoadLocalCall)
p w.shout                # "BOX"
p w.itself2.name         # chained: w.itself2 (fused) then .name (normal)
p w.ghost                # "mm:ghost" (method_missing via fused path)
arr = [3, 1, 2]
p arr.sort               # [1,2,3] (local recv, zero-arg)
p arr.length             # 3
s = "hello"
p s.reverse              # "olleh"
n = nil
begin; n.foo; rescue NoMethodError => e; p :nil_nomethod; end
