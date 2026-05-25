# Basic — `super(arg)` forwards explicit args
class Animal
  def initialize(name)
    @name = name
  end
  def speak
    "#{@name} makes a sound"
  end
end
class Dog < Animal
  def initialize(name, breed)
    super(name)
    @breed = breed
  end
  def speak
    "#{super} (it's a #{@breed} barking)"
  end
end
d = Dog.new("Rex", "Husky")
puts d.speak

# Three levels — `super` walks one step per call, doesn't recurse
class Puppy < Dog
  def speak
    "Yip! " + super
  end
end
puts Puppy.new("Buddy", "Beagle").speak

# Forwarding `super` (bare) — passes the current method's args verbatim
class Greeter
  def greet(name, mood = "happy")
    "#{mood} hello, #{name}"
  end
end
class LoudGreeter < Greeter
  def greet(name, mood = "happy")
    super.upcase
  end
end
puts LoudGreeter.new.greet("ruby")
puts LoudGreeter.new.greet("ruby", "tired")

# `super()` with empty parens — no args
class Base
  def shout
    "BASE"
  end
end
class Child < Base
  def shout
    "[#{super()}]"
  end
end
puts Child.new.shout

# super in initialize chain — call grandparent via the chain
class A
  def initialize
    @log = ["A"]
  end
  def log
    @log
  end
end
class B < A
  def initialize
    super
    @log << "B"
  end
end
class C < B
  def initialize
    super
    @log << "C"
  end
end
c = C.new
puts c.log[0]
puts c.log[1]
puts c.log[2]

# Mixed: super combined with attr_accessor (synthesised setters)
class Box
  attr_accessor :v
  def initialize(v = 0)
    @v = v
  end
end
class CountedBox < Box
  attr_reader :count
  def initialize(v = 0)
    super
    @count = 1
  end
  def v=(new_v)
    @count = @count + 1
    super
  end
end
b = CountedBox.new(10)
puts b.v
puts b.count
b.v = 99
puts b.v
puts b.count
