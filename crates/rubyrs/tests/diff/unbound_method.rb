# Method#unbind / UnboundMethod#bind round-trip. A Method
# captures (recv, name); unbind strips the recv and keeps the
# class, bind rehydrates against a fresh instance of the same
# class (or a subclass).

class Greeter
  def initialize(name); @name = name; end
  def hello(greeting); "#{greeting}, #{@name}"; end
end

g = Greeter.new("Mochi")
m = g.method(:hello)

# unbind keeps class + method-name but loses the receiver.
u = m.unbind
puts u.class.name            # UnboundMethod
puts u.is_a?(UnboundMethod)  # true

# bind against a fresh instance of the same class.
g2 = Greeter.new("Soba")
m2 = u.bind(g2)
puts m2.class.name           # Method
puts m2.call("hola")         # hola, Soba

# Original BoundMethod still works (unbind doesn't mutate).
puts m.call("hi")            # hi, Mochi

# Bind against a subclass instance also works.
class LoudGreeter < Greeter
  def initialize(name); @name = name.upcase; end
end
loud = LoudGreeter.new("Mochi")
m3 = u.bind(loud)
puts m3.call("YO")           # YO, MOCHI

# Mismatched class raises TypeError.
begin
  u.bind("not a greeter")
rescue TypeError => e
  puts "caught: #{e.class.name}"
end
