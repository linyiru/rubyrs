# Bare (implicit-self) `equal?` / `eql?` inside a method dispatch on self.
# dry-core's `Undefined.default` does `def undefined.default(x, y = self);
# if equal?(x)`.
U = Object.new
def U.default(x, y = self)
  if equal?(x) then :x
  elsif equal?(y) then :y
  else x end
end
p U.default(U)
p U.default(42)
p U.default(42, U)
class Box
  def initialize(v); @v = v; end
  def same?(o); equal?(o); end
  def veq?(o); @v.eql?(o); end
end
b = Box.new(7)
p b.same?(b)
p b.same?(Box.new(7))
p b.veq?(7)
