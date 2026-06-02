# Method#original_name / UnboundMethod#original_name — returns
# the Symbol the method was originally `def`'d under, surviving
# `alias_method` indirection. For non-aliased methods,
# `original_name == name`.

class C
  def foo; "C.foo"; end
  alias_method :bar, :foo
  def baz; "C.baz"; end
end

class D < C
  alias_method :qux, :foo
end

c = C.new

# (1) Non-aliased — original_name == name.
puts c.method(:foo).original_name      # foo
puts c.method(:baz).original_name      # baz
puts c.method(:foo).original_name == c.method(:foo).name   # true

# (2) Aliased — name is the alias, original_name is the def name.
puts c.method(:bar).name               # bar
puts c.method(:bar).original_name      # foo

# (3) UnboundMethod parity.
puts C.instance_method(:foo).original_name      # foo
puts C.instance_method(:bar).original_name      # foo
puts C.instance_method(:bar).name               # bar

# (4) Alias-through-inheritance: D aliases :qux → :foo (foo from C).
# The Method record shared with C#foo carries original_name=:foo.
puts D.new.method(:qux).name              # qux
puts D.new.method(:qux).original_name     # foo
puts D.instance_method(:qux).original_name # foo

# (5) Alias of an alias — original_name still tracks the very
# first def name (alias_method shares the same Rc<Method>, so
# the original_name field is preserved through the chain).
class E
  def first; end
  alias_method :second, :first
  alias_method :third, :second
end
puts E.new.method(:second).original_name   # first
puts E.new.method(:third).original_name    # first

# (6) Wrong arity — ArgumentError (CRuby parity, not
# NoMethodError via dispatch fall-through).
begin
  c.method(:foo).original_name(1)
rescue ArgumentError => e
  puts e.message
end

# (7) respond_to? must agree.
puts c.method(:foo).respond_to?(:original_name)         # true
puts C.instance_method(:foo).respond_to?(:original_name) # true
