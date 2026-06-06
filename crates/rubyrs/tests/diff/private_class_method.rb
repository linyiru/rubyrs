# `private_class_method` / `public_class_method` accept method-name
# args and return the receiver. rubyrs doesn't model singleton-method
# visibility, so they are no-ops (a "privatised" class method stays
# callable) — the same documented trade-off as the private_constant
# stub. We assert the return value and that the chain keeps working.
#
# Discovery: P3 Jekyll spike — rubygems, fileutils and
# forwardable-extended all call `klass.private_class_method(...)`
# during their require, in both bareword and explicit-receiver forms.

class Foo
  def self.helper; 42; end
  def self.other;  7;  end

  # bareword form inside the class body — returns the class.
  r = private_class_method(:helper)
  puts r == Foo
end

# explicit-receiver form (the forwardable-extended shape).
r2 = Foo.private_class_method(:other)
puts r2 == Foo

# public_class_method, both forms.
puts(Foo.public_class_method(:helper) == Foo)

class Bar
  def self.a; 1; end
  r = public_class_method(:a)
  puts r == Bar
end

# no-arg form is a no-op returning the receiver.
puts(Foo.private_class_method == Foo)

# Multiple args at once.
class Baz
  def self.x; end
  def self.y; end
  puts(private_class_method(:x, :y) == Baz)
end
