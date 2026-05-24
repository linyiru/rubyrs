class Animal
  def initialize(name)
    @name = name
  end

  def name
    @name
  end

  def describe
    "I am " + @name
  end
end

class Dog < Animal
  def speak
    "woof"
  end
end

class Puppy < Dog
  def cute
    true
  end
end

a = Animal.new("Generic")
puts a.name
puts a.describe

d = Dog.new("Rex")
puts d.name        # inherited
puts d.describe    # inherited
puts d.speak       # own

p = Puppy.new("Tiny")
puts p.name        # inherited from Animal
puts p.describe    # inherited from Animal
puts p.speak       # inherited from Dog
puts p.cute        # own
