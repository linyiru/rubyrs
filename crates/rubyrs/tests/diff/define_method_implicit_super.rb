# Implicit (bare-argument) `super` from a method DEFINED BY
# define_method / define_singleton_method is a RuntimeError in CRuby:
#   "implicit argument passing of super from method defined by
#    define_method() is not supported. Specify all arguments
#    explicitly."
# Previously rubyrs ran the parent body (or raised ArgumentError).
# EXPLICIT-argument super (`super(...)`, `super()`, `super(*a)`) from
# a define_method body is ALLOWED and must keep working; a `def`-body
# bare super (including inside a nested block) must be UNAFFECTED.

def show
  yield
rescue RuntimeError => e
  puts "RuntimeError: #{e.message}"
end

class Base
  def m(*a); "Base#m(#{a.inspect})"; end
  def y; yield; end
end

# (a) bare super, 0-param define_method body → raises
C1 = Class.new(Base) { define_method(:m) { super } }
show { p C1.new.m }

# (b) bare super, define_method body WITH params → still raises
C2 = Class.new(Base) { define_method(:m) { |a| super } }
show { p C2.new.m(1) }

# (c) `super do…end` (implicit args) from define_method → raises
C3 = Class.new(Base) { define_method(:y) { super { 42 } } }
show { p C3.new.y }

# (d) bare super NESTED in an ordinary block inside a define_method → raises
C4 = Class.new(Base) { define_method(:m) { |a| [1].each { super } } }
show { p C4.new.m(3) }

# (e) define_singleton_method bare super → raises
Base2 = Class.new { def self.m; "Base2.m"; end }
C5 = Class.new(Base2)
C5.define_singleton_method(:m) { super }
show { p C5.m }

# EXPLICIT super from define_method is allowed:
# (f) super() with empty parens
C6 = Class.new(Base) { define_method(:m) { super() } }
p C6.new.m                               # "Base#m([])"
# (g) super(arg)
C7 = Class.new(Base) { define_method(:m) { |a| super(a) } }
p C7.new.m(5)                            # "Base#m([5])"
# (h) super(*args) splat
C8 = Class.new(Base) { define_method(:m) { |*a| super(*a) } }
p C8.new.m(7, 8)                         # "Base#m([7, 8])"
# (i) super(arg) do…end (explicit args + explicit block)
C9 = Class.new(Base) { define_method(:y) { super() { 99 } } }
p C9.new.y                               # 99

# bare super from ORDINARY `def` methods is UNAFFECTED:
# (j) plain def bare super forwards
class D1 < Base
  def m(a); super; end
end
p D1.new.m(3)                            # "Base#m([3])"
# (k) bare super nested in a block inside a plain def method
class D2 < Base
  def m(a); [1].map { super }; end
end
p D2.new.m(4)                            # ["Base#m([4])"]
