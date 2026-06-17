# Module#singleton_class? — true iff the receiver is an eigenclass (the
# metaclass of some object or class), false for an ordinary class/module.
# sorbet-runtime's method-hook installer skips singleton classes via
# `mod.singleton_class?`.
p Class.new.singleton_class?                      # false
p String.singleton_class?                         # false
p Module.new.singleton_class?                     # false
p Class.singleton_class?                          # false
p Object.singleton_class?                         # false

# eigenclasses ARE singleton classes
p Object.new.singleton_class.singleton_class?     # true
p String.singleton_class.singleton_class?         # true
p Class.singleton_class.singleton_class?          # true

# a named class and a module
class Foo; end
module Bar; end
p Foo.singleton_class?                            # false
p Bar.singleton_class?                            # false
p Foo.singleton_class.singleton_class?            # true

# the eigenclass reached via `class << self`
got = nil
class Baz
  class << self
    self
  end
end
o = Object.new
sc = (class << o; self; end)
p sc.singleton_class?                             # true
