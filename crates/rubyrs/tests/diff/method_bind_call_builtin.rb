# UnboundMethod#bind_call / BoundMethod#bind_call must work for NATIVE
# builtin instance methods (which have no Ruby-level method object) by
# dispatching the method by name on the target.
um = String.instance_method(:upcase)
p um.bind_call("hi")
p um.bind("hi").call
p Integer.instance_method(:to_s).bind_call(255, 16)
p Array.instance_method(:first).bind_call([1, 2, 3])
p Array.instance_method(:first).bind_call([1, 2, 3], 2)

# Round-trip through unbind.
m = "x".method(:upcase)
p m.unbind.bind_call("yz")

# A user-defined method still works (regression guard for the
# table-Method path).
class Foo
  def greet(n); "hi #{n}"; end
end
p Foo.instance_method(:greet).bind_call(Foo.new, "bob")

# bind_call can reach a private method.
class Bar
  private def secret; 42; end
end
p Bar.instance_method(:secret).bind_call(Bar.new)
