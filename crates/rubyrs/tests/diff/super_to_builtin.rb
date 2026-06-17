# `super` from a method that overrides a universal/primitive BUILTIN must reach
# the native builtin (the super chain has no user method above). sequel's
# `Dataset#freeze` does `def freeze; …; super; end`.
class A
  def freeze; @froze = true; super; end
  def frozen_marker; @froze; end
end
a = A.new
p a.freeze.equal?(a)   # true (freeze returns self)
p a.frozen?            # true (builtin freeze ran)
p a.frozen_marker      # true

class B
  attr_reader :calls
  def initialize; @calls = 0; end
  def ==(o); @calls += 1; super; end
end
b = B.new
p (b == b)             # true (super -> builtin identity ==)
p (b == B.new)         # false
p b.calls              # 2

class C
  def inspect; "wrapped:" + super; end
end
p C.new.inspect.start_with?("wrapped:#<C")  # true

# super to to_s
class D
  def to_s; "D[" + super + "]"; end
end
p D.new.to_s.start_with?("D[#<D")            # true
