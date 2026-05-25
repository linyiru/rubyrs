# `defined?(expr)` — returns a string describing the kind of
# expression, or nil if it would raise NameError / NoMethodError.
# Resolved at AST translation: literals get static labels;
# ivars / methods / constants go through Kernel helpers that
# inspect runtime state.

# Literals are always "expression" / explicit-keyword labels.
p defined?(1)
p defined?(1.5)
p defined?("hello")
p defined?(:foo)
p defined?(nil)
p defined?(true)
p defined?(false)
p defined?(self)

# Local variable (parser only emits LocalVariableReadNode when
# a local is in scope, so this is statically "local-variable").
x = 1
p defined?(x)

# Method name (runtime lookup against builtin / host / class
# methods / toplevel methods).
p defined?(puts)
p defined?(p)
p defined?(Integer)         # `Integer()` is a Kernel function;
                            # but Integer is also a constant — CRuby
                            # picks the constant. Documented divergence:
                            # we resolve it as "constant" via the
                            # class table.
p defined?(undefined_method)

# Constant reference — class-table lookup.
p defined?(Foo)
p defined?(String)          # built-in stub class
p defined?(Array)
p defined?(NoSuchConst)

# Instance variable — checks the current self.
class Person
  def initialize(name)
    @name = name
  end

  def name_set?
    defined?(@name)
  end

  def age_set?
    defined?(@age)
  end
end

alice = Person.new("Alice")
p alice.name_set?
p alice.age_set?

# defined? on a method call with args — falls through to
# "expression" in our subset (CRuby would also check arg
# definedness recursively; documented divergence).
puts defined?(1 + 2)

# defined? wrapping a method call inside a defined() chain.
puts defined?(puts("hi"))

# Used in a conditional guard.
def safe_method?(name)
  defined?(name).nil? ? "missing" : "present"
end
# `name` IS a local-variable in this body, so defined?(name)
# is "local-variable".
puts safe_method?("x")

# `defined?` returns nil for missing ivars even on a class that
# defines other ivars.
class Box
  def initialize
    @has = "set"
  end
  def status(probe)
    if probe == :has
      defined?(@has) || "miss"
    else
      defined?(@nope) || "miss"
    end
  end
end

b = Box.new
puts b.status(:has)
puts b.status(:nope)
