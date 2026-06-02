# Method#name / UnboundMethod#name — returns the captured
# method-name Symbol. Same shape for bound and unbound; for
# aliased methods the captured name is reported (CRuby parity).

class C
  def foo; end
  def bar(x, y); end
  alias_method :baz, :foo
end

class D < C
  def own; end
end

# (1) Bound, own method
m = C.new.method(:foo)
puts m.name
puts m.name.class

# (2) Bound, multi-arg
puts C.new.method(:bar).name

# (3) Unbound
puts C.instance_method(:foo).name
puts C.instance_method(:bar).name

# (4) Inherited — name is what was captured, owner walks to defining class
inh = D.new.method(:foo)
puts inh.name
puts inh.owner.name

# (5) Aliased — captured-name semantics: name reports what
# `.method(:baz)` was asked for, not the original target.
puts C.new.method(:baz).name
puts C.instance_method(:baz).name

# (6) Singleton
o = Object.new
def o.sing; end
puts o.method(:sing).name

# (7) Round-trip — name fed back into method(...) yields a
# Method with the same name Symbol.
m1 = C.new.method(:foo)
m2 = C.new.method(m1.name)
puts m1.name == m2.name

# (8) respond_to?
puts C.new.method(:foo).respond_to?(:name)
puts C.instance_method(:foo).respond_to?(:name)
