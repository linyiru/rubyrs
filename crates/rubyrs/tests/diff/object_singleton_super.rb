# `super` from a per-object singleton method defined with `def self.x`
# in a context with NO enclosing class body (e.g. inside a block /
# method body — minitest's `it` block does `def self.env; super; end`).
# The singleton method lives on the object's eigenclass, so `super`
# must resume from the eigenclass's superclass (the object's real
# class). Previously `defining_class` was taken from the (empty) class
# stack, so `super` couldn't locate its start point.

class Base
  def helper(x); "base:#{x}"; end
end

# (a) define the singleton inside instance_eval (self = the object,
# empty class stack), then invoke from a clean stack.
sub = Class.new(Base)
o = sub.new
o.instance_eval do
  def self.helper(x)
    super(x * 2)
  end
end
p o.helper(5)              # "base:10"

# (b) define inside a define_method'd block body (the minitest shape)
klass = Class.new(Base)
klass.send(:define_method, :run) do
  def self.helper(x)
    super(x + 100)
  end
  helper(7)
end
p klass.new.run            # "base:107"

# (c) the singleton overrides, supers, AND the eigenclass keeps other
# inherited methods reachable
m = Class.new(Base).new
def m.helper(x); "wrap(" + super(x) + ")"; end
p m.helper(1)              # "wrap(base:1)"

# (d) a deeper chain: super walks past the immediate class to a module
module Mixin
  def greet(n); "mix:#{n}"; end
end
deep = Class.new { include Mixin }
d = deep.new
def d.greet(n); super(n * 3); end
p d.greet(4)               # "mix:12"
