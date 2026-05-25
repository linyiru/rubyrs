# `Object#method(:name)` returns a Method-shaped Value that
# captures (receiver, method_name) and can be called later via
# `.call(args)` / `.()` / `[args]`.

class Greeter
  def initialize(name); @name = name; end
  def hello(greeting); "#{greeting}, #{@name}"; end
  def shout(text); "#{text}!"; end
end

g = Greeter.new("Mochi")

# Basic capture + .call
m = g.method(:hello)
puts m.call("hi")
puts m.call("hola")

# Call syntax aliases.
puts m.("welcome")
puts m["sup"]

# Storing multiple methods.
greet = g.method(:hello)
yell  = g.method(:shout)
puts greet.call("hey")
puts yell.call("LOUD")

# Method on a primitive receiver.
n = 7
plus = n.method(:+)
puts plus.call(3)
puts plus.(10)

# `&method-object` forwarding requires implicit `to_proc`
# coercion which is a deferred feature (SUBSET.md). Stored-and-
# invoked instead.

# Stored on an instance, dispatched later.
class Dispatcher
  def initialize(target, method_name)
    @m = target.method(method_name)
  end
  def call_with(arg)
    @m.call(arg)
  end
end
d = Dispatcher.new(g, :hello)
puts d.call_with("dispatched")

# Type name. `m.class.name == "Method"` works since K9
# registered the Method class in the preamble.
puts m.class.name
puts m.is_a?(Method)
