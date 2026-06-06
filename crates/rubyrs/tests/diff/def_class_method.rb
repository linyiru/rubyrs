# `def Foo.bar` / `def Foo::bar` (explicit class-constant receiver)
# defines a CLASS method on Foo, like `def self.bar`. Discovery: P3
# Jekyll spike — rexml's `def SourceFactory::create_from(arg)`.
class Foo
  def Foo.create(x); "created:#{x}"; end
  def Foo::build(y); "built:#{y}"; end
  def self.via_self; "self"; end
end
p Foo.create("a")
p Foo.build("b")
p Foo.via_self
p Foo.respond_to?(:create)
p Foo.respond_to?(:build)

# outside the class body
class Bar; end
def Bar.make(n); n * 2; end
p Bar.make(21)

# nested-module constant receiver
module M
  class Widget
    def Widget.spawn; "spawned"; end
  end
end
p M::Widget.spawn

# the class method can reference the class + call siblings
class Counter
  def Counter.zero; new(0); end
  def initialize(n); @n = n; end
  def value; @n; end
end
p Counter.zero.value
