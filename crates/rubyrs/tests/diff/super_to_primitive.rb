# `super` resolving to a builtin Class / BasicObject method that
# rubyrs handles inline (so the ancestor walk finds no user
# Method above the override). CRuby ships real `Class#new`,
# `Class#allocate`, `BasicObject#initialize`, so an overriding
# `def self.new` / `def initialize` can call `super`. Pre-fix
# rubyrs raised `NoMethodError: super: no superclass method`.
#
# Discovery: P3 Sinatra spike -- Mustermann's
# `def self.new(...); ...; super(string, **options) { options }
# end` (pattern.rb) and Sinatra::Templates' `def initialize;
# super; end` both depend on it.

# Shape 1: `def self.new` override calling super -> builtin
# Class#new allocates an instance of the RECEIVER class. Use a
# source-form subclass so the class has a stable name (anon
# Class.new naming is a separate documented divergence).
class Base1
  def self.new(*args)
    super
  end
end
class Sub1 < Base1; end
x = Sub1.new
puts "shape1_class=#{x.class}"
puts "shape1_isa_sub=#{x.is_a?(Sub1)}"
puts "shape1_isa_base=#{x.is_a?(Base1)}"

# Shape 2: super in `def self.new` forwards args to initialize.
class Box
  def self.new(*args)
    super
  end
  def initialize(v)
    @v = v
  end
  def v; @v; end
end
puts "shape2_v=#{Box.new(42).v}"

# Shape 3: `def initialize; super; end` chains to an inherited
# initialize (the classic Animal/Dog shape).
class Animal
  def initialize(name)
    @name = name
  end
  def name; @name; end
end
class Dog < Animal
  def initialize(name, breed)
    super(name)
    @breed = breed
  end
  def breed; @breed; end
end
d = Dog.new("Rex", "Lab")
puts "shape3_name=#{d.name}"
puts "shape3_breed=#{d.breed}"

# Shape 4: `super` with no parent initialize falls through to the
# BasicObject#initialize no-op (returns nil, doesn't raise).
class Plain
  def initialize
    super
    @ok = true
  end
  def ok; @ok; end
end
puts "shape4_ok=#{Plain.new.ok}"

# Shape 5: bare `super` (implicit args) in `def self.new`.
class Bare
  def self.new
    super
  end
  def initialize
    @made = :yes
  end
  def made; @made; end
end
puts "shape5_made=#{Bare.new.made}"

# Shape 6: a deeper override chain -- Grand.new overrides, Child
# inherits the override, super still reaches the builtin.
class Grand
  def self.new(*a)
    super
  end
end
class Child < Grand
  def initialize
    @from = :child
  end
  def from; @from; end
end
puts "shape6_from=#{Child.new.from}"
puts "shape6_class=#{Child.new.class}"

# Shape 7: `def self.allocate; super; end` -> builtin
# Class#allocate produces a bare instance (no initialize) of the
# receiver class.
class Alloc1
  def self.allocate
    super
  end
end
class Alloc2 < Alloc1; end
bare = Alloc2.allocate
puts "shape7_class=#{bare.class}"
puts "shape7_isa=#{bare.is_a?(Alloc2)}"
