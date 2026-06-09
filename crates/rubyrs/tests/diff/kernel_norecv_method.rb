# A method added to Kernel (by reopening `module Kernel`) is callable via
# IMPLICIT self from any context — every object's ancestry includes
# Kernel. Bare `Foo(x)` resolved with a nil/class receiver before this.
# (This is how the Kernel#BigDecimal() conversion function works.)
module Kernel
  def Twice(x); x * 2; end
  def shout(s); s.upcase + "!"; end
end
# toplevel (self is the main object)
p Twice(21)                    # 42
p shout("hi")                  # "HI!"
# inside an instance method body (self is an Object)
class A
  def go; Twice(5); end
end
p A.new.go                     # 10
# inside a class-method body / module function (self is a Class) — the
# liquid `def self.to_number; BigDecimal(...); end` shape
class B
  def self.compute; Twice(100); end
end
p B.compute                    # 200
module M
  def self.helper; shout("mod"); end
end
p M.helper                     # "MOD!"
# bare def at toplevel still wins (own method before Kernel)
def Twice(x); x * 3; end
p Twice(10)                    # 30 (toplevel override)
