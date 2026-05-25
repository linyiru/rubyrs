# Module / include / extend — our subset treats modules as
# classes-without-superclass, so `include` and `extend` route
# through the same method-copy machinery. What's lost vs CRuby:
# the strict "modules can't be instantiated" check and full
# ancestor-chain introspection. What works: defining shared
# behaviour and mixing it into multiple classes.

module Greetable
  def greet
    "hi from #{self.class.name}"
  end
end

class Robot
  include Greetable
end

class Person
  include Greetable
end

puts Robot.new.greet
puts Person.new.greet

# Two modules in the same class.
module Walkable
  def walk
    "walking"
  end
end

module Runnable
  def run
    "running"
  end
end

class Athlete
  include Greetable
  include Walkable
  include Runnable
end

a = Athlete.new
puts a.greet
puts a.walk
puts a.run

# Class's own methods override the included module's.
module Verbose
  def hi
    "module hi"
  end
end

class Quiet
  include Verbose
  def hi
    "class hi"
  end
end

puts Quiet.new.hi

# Module method delegates to self — exercises the
# "late-binding self" property of method copy.
module Inspector
  def describe
    "#{self.class.name}(#{value})"
  end
end

class Wrapper
  include Inspector
  def initialize(v); @v = v; end
  def value; @v; end
end

puts Wrapper.new(42).describe
puts Wrapper.new("text").describe

# Module that uses Comparable internally.
module SizedComparable
  include Comparable
  def <=>(other)
    size <=> other.size
  end
end

class Box
  include SizedComparable
  attr_reader :size
  def initialize(s); @size = s; end
end

small = Box.new(1)
big = Box.new(10)
puts small < big
puts big > small
puts small.between?(Box.new(0), Box.new(5))

# Sort an Array of Boxes.
boxes = [Box.new(3), Box.new(1), Box.new(2)].sort
puts boxes.map(&:size).inspect

# Module method that calls a method the including class is
# expected to define.
module Countable
  def empty?
    count == 0
  end
  def any?
    count > 0
  end
end

class Bag
  include Countable
  def initialize(items); @items = items; end
  def count; @items.length; end
end

puts Bag.new([]).empty?
puts Bag.new([1, 2]).empty?
puts Bag.new([]).any?
puts Bag.new([1]).any?

# A module's name is still queryable.
puts Greetable.name
