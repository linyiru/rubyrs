# `class << X; def name; ... end; end` — singleton class body
# with def-only contents. Spike scope: methods defined this way
# land on X's singleton method table, same as `def X.name`.
# Both rubyrs and CRuby must produce identical stdout.

# `class << self` inside a class body — adds class methods.
class Foo
  class << self
    def hello
      "from Foo's singleton"
    end
    def shout(s)
      "#{s}!"
    end
  end
end
puts Foo.hello
puts Foo.shout("hi")

# Multiple defs in the body, including one with optional args.
class Calc
  class << self
    def zero
      0
    end
    def double(n)
      n * 2
    end
    def add(a, b = 10)
      a + b
    end
  end
end
puts Calc.zero
puts Calc.double(7)
puts Calc.add(5)
puts Calc.add(5, 20)

# `class << obj` on an instance — methods land on the eigenclass.
class Bar; end
b = Bar.new
class << b
  def greet
    "instance-singleton greeting"
  end
end
puts b.greet
# Second instance of Bar doesn't have the singleton method.
b2 = Bar.new
puts b2.respond_to?(:greet)
puts b.respond_to?(:greet)

# Tilt-shape: ivar-using class method.
class Box
  class << self
    def metadata
      @metadata ||= { color: :red }
    end
    def default_color
      metadata[:color]
    end
  end
end
puts Box.default_color
puts Box.metadata.inspect
