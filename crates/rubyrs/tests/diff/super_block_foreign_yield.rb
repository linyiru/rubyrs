# `super` inside a block forwards to the enclosing METHOD's super-chain,
# resolved LEXICALLY (the method that textually contains the `super`),
# NOT by the call-stack-nearest method frame. They diverge when the
# block is invoked through a method on ANOTHER object — the shape
# liquid's TableRow#render uses:
#   context.stack { collection.each { result << super } }
# The intervening `Context#stack` frame's defining_class is Context,
# which isn't in self's ancestry, so picking it raised a spurious
# "super: no superclass method".

class A; def f(x); "A#{x}"; end; end

class Yielder; def go; yield; end; end

# block invoked by another object's method, then a native each inside
class D < A
  def f(x)
    r = ""
    Yielder.new.go { [1, 2].each { r << super } }
    r
  end
end
p D.new.f(5)

# class-method super through a foreign yielder
class E; def self.g(n); "E#{n}"; end; end
class G < E
  def self.g(n)
    r = ""
    Yielder.new.go { r << super }
    r
  end
end
p G.g(3)

# explicit-arg super inside the foreign-yielded block
class H < A
  def f(x)
    r = ""
    Yielder.new.go { [7, 8].each { |y| r << super(y) } }
    r
  end
end
p H.new.f(0)

# module-include super through a foreign yielder
module M; def f(x); "M#{x}"; end; end
class N
  include M
  def f(x)
    r = ""
    Yielder.new.go { r << super }
    r
  end
end
p N.new.f(4)
