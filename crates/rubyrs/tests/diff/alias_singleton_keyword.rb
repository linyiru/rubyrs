# `alias new old` keyword form INSIDE `class << X` body.
# CRuby installs the alias on X's singleton class so both
# `new` and `old` are callable as class methods.
#
# PR #96 handled the regular-context form (top-level / normal
# class body — emits Op::AliasMethod against instance methods).
# This fixture covers the singleton context: AST detects the
# alias inside `class << X` body and emits the new
# Op::AliasSingletonMethod, which installs into
# `class_stack.last().singleton_methods` instead.

class Tilt
  def self.register
    "registered"
  end
  def self.lazy_map
    "lazy"
  end
  class << self
    alias prefer register
    alias register_lazy lazy_map
  end
end

# Both new aliases callable as class methods.
puts Tilt.register
puts Tilt.prefer
puts Tilt.lazy_map
puts Tilt.register_lazy

# Mixed with attr_accessor (which already worked from PR #94)
# and a `def self.x` sibling. The aliases must coexist with
# the rest of the class << self body without interference.
class Greeter
  def self.hi; "hi"; end
  def self.bye; "bye"; end
  class << self
    attr_accessor :version
    alias hello hi
    alias goodbye bye
    def shout; "SHOUT"; end
  end
end

puts Greeter.hi
puts Greeter.hello
puts Greeter.bye
puts Greeter.goodbye
puts Greeter.shout
puts Greeter.version.inspect

# Inherited singleton method alias — `def self.parent_method`
# on Base, `alias child_alias parent_method` in `class << self`
# inside Child. Should walk Base's singleton chain via
# `lookup_class_singleton_method`.
class Base
  def self.parent_method; "from Base"; end
end
class Child < Base
  class << self
    alias child_alias parent_method
  end
end
puts Child.child_alias
