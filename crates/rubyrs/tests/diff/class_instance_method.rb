# Class#instance_method(:sym) — direct UnboundMethod construction.
# Equivalent to `C.new.method(:sym).unbind` but doesn't allocate
# a throwaway instance.

class Greeter
  def hello(name); "hi, #{name}"; end
  def shout(s); "#{s}!"; end
end

# Basic lookup.
u = Greeter.instance_method(:hello)
puts u.class.name                       # UnboundMethod
puts u.is_a?(UnboundMethod)             # true

# bind + call round-trip.
g = Greeter.new
m = u.bind(g)
puts m.call("Mochi")                    # "hi, Mochi"
puts m.class.name                       # Method

# Two instance methods compare unequal.
u_hello = Greeter.instance_method(:hello)
u_shout = Greeter.instance_method(:shout)
puts u_hello == u_shout                 # false

# Same method twice -> equal.
puts u_hello == Greeter.instance_method(:hello)   # true

# Inherited methods resolve via the chain. Both C and D's
# `instance_method(:hello)` produce equal UnboundMethods because
# Method#== compares the underlying Method record.
class LoudGreeter < Greeter
end
puts Greeter.instance_method(:hello) == LoudGreeter.instance_method(:hello)   # true

# NameError on missing.
begin
  Greeter.instance_method(:nope)
rescue NameError => e
  puts "caught: #{e.class.name}"
end
