# A bare `inspect` / `to_s` (implicit self) inside an instance method
# dispatches on self — like the other bare universals (dup, frozen?,
# methods). Was NoMethodError. A `pretty_inspect` that delegates to
# bare `inspect` is the motivating shape.

class Plain
  def via_inspect; inspect; end
  def via_to_s;    to_s;    end
end
pl = Plain.new
p pl.via_inspect.start_with?("#<Plain")   # default inspect
p pl.via_to_s.start_with?("#<Plain")      # default to_s

# User overrides are honoured first.
class Custom
  def inspect; "INSPECTED"; end
  def to_s;    "STRINGED";  end
  def a; inspect; end
  def b; to_s;    end
  def c; "wrap(#{to_s})"; end   # interpolation of a bare to_s
end
cu = Custom.new
p cu.a
p cu.b
p cu.c

# Class self: bare inspect/to_s inside a class method.
class WithClassMethods
  def self.describe; inspect; end
end
p WithClassMethods.describe                # "WithClassMethods"

# Toplevel bare to_s (self = main).
p to_s

# pretty_inspect-style delegation to bare inspect.
class Object
  def my_pretty; inspect; end
end
p [1, 2].my_pretty                         # "[1, 2]"
p({a: 1}.my_pretty)                        # "{a: 1}"-ish
