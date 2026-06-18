# define_singleton_method(name, callable) on a heap primitive (Array /
# String) installs onto its per-instance eigenclass — Tilt binds a
# compiled UnboundMethod onto an Array this way.
class Array
  def __dsm_probe; "arr:#{size}"; end
end
um = Array.instance_method(:__dsm_probe)
a = [1, 2, 3]
p a.define_singleton_method(:probe, um)
p a.probe
b = [9]
p b.respond_to?(:probe)   # singleton, not shared with other arrays

# block form still works on Array
c = []
c.define_singleton_method(:blk) { "blk-ok" }
p c.blk

# String eigen with a String-scoped UnboundMethod
class String
  def __dsm_s; "str:#{length}"; end
end
sm = String.instance_method(:__dsm_s)
s = "hello".dup
s.define_singleton_method(:probe, sm)
p s.probe
