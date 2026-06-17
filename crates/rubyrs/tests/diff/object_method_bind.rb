# `Object.instance_method(:method)` resolves the Kernel `method` builtin,
# and an UnboundMethod rooted at Object/BasicObject/Kernel binds to ANY
# receiver (every value is an Object). Surfaced by sorbet-runtime's
# `Object.instance_method(:method).bind_call(mod, :new).owner` check.
p Object.instance_method(:method).class          # UnboundMethod
p Object.instance_method(:method).name           # :method

class Bar; end
p Bar.method(:new).class                          # Method
# bind_call an Object-rooted unbound method onto a Class instance
m = Object.instance_method(:method).bind_call(Bar, :new)
p m.class                                          # Method
p m.name                                           # :new

# bind onto a plain object too
um = Object.instance_method(:method)
p um.bind(Bar).class                               # Method
p um.bind("a string").class                        # Method (String is an Object)
p um.bind(42).class                                # Method (Integer is an Object)

