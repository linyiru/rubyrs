# Class#instance_method on a primitive class (Integer / Float /
# String / etc.) no longer raises NameError just because the
# method isn't in the class's user-Method table. Primitives
# dispatch through `primitive_call` / `numeric_call`, not the
# methods table, so the lookup would always miss.
#
# A6c relaxes the lookup: for the well-known primitive class
# names, synthesise an UnboundMethod even when the methods
# table doesn't contain the symbol. Downstream `arity` /
# `parameters` arms already fall back to the builtin sentinel
# (arity = -1, parameters = [[:rest]]) when the Method record
# is absent, so the synthetic UnboundMethod answers metadata
# queries with the documented "unknown / variadic" shape.
#
# User classes still raise NameError on unknown methods — the
# safety-critical "typo detection" path is unchanged.

# Primitive: produces an UnboundMethod with arity = -1.
m = Integer.instance_method(:[])
puts m.class.name                        # UnboundMethod

# Other primitives: same treatment.
m2 = String.instance_method(:length)
puts m2.class.name                       # UnboundMethod
m3 = Array.instance_method(:push)
puts m3.class.name                       # UnboundMethod
m4 = Hash.instance_method(:keys)
puts m4.class.name                       # UnboundMethod

# User class still returns the real UnboundMethod for a known
# method...
class Greeter
  def hello(name); "hi, #{name}"; end
end
puts Greeter.instance_method(:hello).class.name  # UnboundMethod
puts Greeter.instance_method(:hello).arity       # 1

# ...and still raises NameError for an unknown method.
begin
  Greeter.instance_method(:nonexistent)
rescue NameError => e
  puts "user-class NameError: caught"
end

# The msgpack-bigint motivation: `Integer.instance_method(:[]).arity`
# is checked by the gem to detect Ruby ≥ 2.7's two-arg [] form.
# CRuby returns -1 (variadic); we also return -1 via the builtin
# fallback. The comparison `!= 1` is what the gem cares about.
puts Integer.instance_method(:[]).arity != 1   # true
