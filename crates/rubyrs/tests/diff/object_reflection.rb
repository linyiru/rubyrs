# Object#methods and Object#instance_variables — basic
# reflection. For user-class instances, methods walks the class
# chain; for primitives the subset returns [] (no per-Kernel-
# method enumeration).

class Greeter
  def hello(name); "hi, #{name}"; end
  def shout(s); "#{s}!"; end
  def initialize(name = "default")
    @name = name
    @count = 0
  end
end

g = Greeter.new("Mochi")

# methods: includes own + inherited.
puts g.methods.include?(:hello)         # true
puts g.methods.include?(:shout)         # true
puts g.methods.include?(:nope)          # false

# Subclass inherits parent's methods.
class LoudGreeter < Greeter
  def yell; "AAAH"; end
end
lg = LoudGreeter.new("Bob")
puts lg.methods.include?(:yell)         # true
puts lg.methods.include?(:hello)        # true (inherited)
puts lg.methods.include?(:shout)        # true

# instance_variables: shows @-prefixed Symbols.
puts g.instance_variables.sort.inspect  # [:@count, :@name]
puts g.instance_variables.length        # 2

# Empty for primitives.
puts({}.instance_variables.inspect)     # []
puts 5.instance_variables.inspect       # []
puts "x".instance_variables.inspect     # []
puts [].instance_variables.inspect      # []
puts :sym.instance_variables.inspect    # []

# methods on primitives returns [] in the subset; CRuby would
# list every Kernel/Numeric/Comparable method (~150 names). The
# divergence is documented in SUBSET.md; not exercised here so
# the fixture stays diff-stable.
