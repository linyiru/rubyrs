# Method#== / UnboundMethod#==
# - BoundMethod: same receiver identity AND same method name.
# - UnboundMethod: same underlying Method definition (so a method
#   inherited by a subclass equals the parent's UnboundMethod).

class C
  def foo; end
  def bar; end
end
class D < C
end

c1 = C.new
c2 = C.new
d1 = D.new

# Same recv + same name → true (different .method calls produce
# different BoundMethod ObjIds but equal value).
puts c1.method(:foo) == c1.method(:foo)   # true

# Different recv of the same class → false.
puts c1.method(:foo) == c2.method(:foo)   # false

# Same recv, different name → false.
puts c1.method(:foo) == c1.method(:bar)   # false

# Subclass instance inherits foo but its recv differs from c1 — false.
puts c1.method(:foo) == d1.method(:foo)   # false

# Cross-type — false, no NoMethodError.
puts c1.method(:foo) == 42                # false

# Primitive recv compares by value: 7.method(:+) on two literal
# 7s is the "same" Integer.
puts 7.method(:+) == 7.method(:+)         # true

# UnboundMethod equality: same underlying definition. (We reach
# UnboundMethods via `bm.unbind` since `Class#instance_method`
# isn't in the subset.)
u_foo_via_c1 = c1.method(:foo).unbind
u_foo_via_c2 = c2.method(:foo).unbind
u_foo_via_d  = d1.method(:foo).unbind
u_bar        = c1.method(:bar).unbind

puts u_foo_via_c1 == u_foo_via_c2   # true
puts u_foo_via_c1 == u_foo_via_d    # true — D inherits :foo from C
puts u_foo_via_c1 == u_bar          # false
