# BasicObject#singleton_method_added(name) — fires after every
# singleton-method install on the receiver. CRuby parity: this is
# the singleton-method twin of Module#method_added (PR #362).
# Rails / RSpec / many DSLs use it to auto-wrap class methods.

# (1) `def self.foo` on a class fires C.singleton_method_added(:foo).
class A
  def self.singleton_method_added(name)
    puts "A.sma(#{name})"
  end
  def self.foo; end
  def self.bar; end
end

# (2) `def obj.foo` on an instance fires obj.singleton_method_added(:foo).
# The hook is a regular instance method on obj's class.
class B
  def singleton_method_added(name)
    puts "B#sma(#{name})"
  end
end
b = B.new
def b.hi; end
def b.there; end

# (3) `obj.define_singleton_method(:foo) { ... }` (block form) on
# an Object receiver fires the hook too.
class C
  def singleton_method_added(name)
    puts "C#sma(#{name})"
  end
end
c = C.new
c.define_singleton_method(:via_block) { "block" }

# (4) `Klass.define_singleton_method(:foo) { ... }` on a Class
# receiver fires Klass.singleton_method_added(:foo).
class D
  def self.singleton_method_added(name)
    puts "D.sma(#{name})"
  end
end
D.define_singleton_method(:cls_method) { "ok" }

# (5) Hook receiver identity inside the body: for Class recv
# `self == ClassName`; for Object recv `self == obj`.
class E
  def self.singleton_method_added(name)
    puts "self == E : #{self == E}"
  end
  def self.first_method; end
end

class F
  def singleton_method_added(name)
    puts "self.is_a?(F): #{self.is_a?(F)}"
  end
end
f = F.new
def f.hello; end

# (6) No hook defined — silent no-op (CRuby doesn't raise).
class G; end
g = G.new
def g.also_lonely; end
puts "G done"

# (7) 2-arg form `Klass.define_singleton_method(:foo, callable)`
# fires the hook too.
class H
  def self.singleton_method_added(name)
    puts "H.sma(#{name})"
  end
  def self.original; "orig"; end
end
H.define_singleton_method(:aliased, H.method(:original))
