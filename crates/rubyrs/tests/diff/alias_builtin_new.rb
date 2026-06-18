# `alias new! new` snapshots the builtin Class#new, so a later
# `def new` that calls `new!` reaches the allocator (no recursion) —
# Sinatra's middleware-wrapping `new`. CRuby's alias-snapshot semantics.
class Foo
  class << self
    alias new! new
    def new(*a, &b)
      inst = new!(*a, &b)
      "wrapped:#{inst.class}:#{inst.x}"
    end
  end
  def initialize(x = 7); @x = x; end
  def x; @x; end
end
p Foo.new
p Foo.new(99)
# new! bypasses the wrapper (raw instance)
raw = Foo.new!(5)
p [raw.class.name, raw.x]
# subclass still works
class Bar < Foo; end
p Bar.new(3)
