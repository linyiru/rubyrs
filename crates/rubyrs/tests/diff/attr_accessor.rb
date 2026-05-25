# Single attr_accessor — getter + setter
class Box
  attr_accessor :value
end
b = Box.new
puts b.value.nil?
b.value = 42
puts b.value
b.value = "hello"
puts b.value

# Multiple attrs in one call
class Point
  attr_accessor :x, :y, :z
  def initialize(x, y, z)
    @x = x
    @y = y
    @z = z
  end
end
p = Point.new(1, 2, 3)
puts p.x
puts p.y
puts p.z
p.x = 10
puts p.x

# attr_reader — getter only
class Stamp
  attr_reader :created_at
  def initialize(t)
    @created_at = t
  end
end
s = Stamp.new("2026-05-24")
puts s.created_at
puts s.respond_to?(:created_at)
puts s.respond_to?(:created_at=)   # false — writer not defined

# attr_writer — setter only
class Secret
  attr_writer :code
  def reveal
    @code
  end
end
sec = Secret.new
sec.code = "1234"
puts sec.reveal
puts sec.respond_to?(:code)        # false — reader not defined
puts sec.respond_to?(:code=)

# Setter returns the assigned value (CRuby semantics — the setter
# method's return value is ignored at the call site; the
# *assignment expression* evaluates to the RHS)
class Cell
  attr_accessor :v
end
c = Cell.new
x = (c.v = 99)
puts x
puts c.v

# Inheritance + attr_accessor — child can read parent's attrs
class Animal
  attr_accessor :name
end
class Dog < Animal
  attr_accessor :breed
  def initialize(name, breed)
    @name = name
    @breed = breed
  end
end
d = Dog.new("Rex", "Husky")
puts d.name
puts d.breed
d.name = "Buddy"
puts d.name

# attr_accessor + method that uses both via `self.` (or implicit)
class Counter
  attr_accessor :count
  def initialize
    @count = 0
  end
  def bump
    @count = @count + 1
    self
  end
end
ctr = Counter.new
ctr.bump.bump.bump
puts ctr.count

# Combining attr_accessor with default args (sanity check both
# features compose)
class Greeter
  attr_accessor :name
  def initialize(name = "world")
    @name = name
  end
  def greeting(prefix = "hi")
    "#{prefix}, #{@name}"
  end
end
puts Greeter.new.greeting
puts Greeter.new("ruby").greeting("hey")
g = Greeter.new
g.name = "rs"
puts g.greeting
