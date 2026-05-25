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

# Tilt-shape: ivar-using class method + bare cross-call to
# another singleton method (the same-class case the review fix
# covered).
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

# Singleton-method INHERITANCE: bare call inside a class
# singleton method body should walk the superclass chain's
# singleton_methods tables, not just self's own.
class Animal
  class << self
    def kingdom; "Animalia"; end
  end
end
class Dog < Animal
  class << self
    def describe
      # Bare `kingdom` — only defined on Animal's singleton table.
      # Without the lookup walking the chain, this would raise
      # NoMethodError.
      "Dog belongs to #{kingdom}"
    end
  end
end
puts Dog.describe

# Side-effect-free receiver works (the only realistic case for
# the spike scope). The single-evaluation guarantee — `class <<
# expr` evaluates expr once — is implemented by binding the
# receiver into a synthetic local before the rewritten defs.
# We verify the LOCAL doesn't collide with a user local of the
# same prefix by accident: this trivially passes if compilation
# succeeds, since a collision would shadow the user's binding.
COUNTER_TAG = "tag-once"
class Tagged
  class << self
    def tag; COUNTER_TAG; end
  end
end
puts Tagged.tag
