# `p` calls a user `inspect`, `puts`/`print` call a user `to_s` (CRuby).
# Previously they used native conversion and ignored user overrides.

class C
  def inspect; "CUSTOM_INSPECT"; end
  def to_s; "CUSTOM_TOS"; end
end

p C.new
puts C.new
print C.new
puts

# Struct / Data now print correctly via p (their inspect is dispatched)
S = Struct.new(:a, :b)
p S.new(1, 2)
D = Data.define(:x, :y)
p D.new(3, 4)

# native types unchanged
p 42
p "hi"
p :sym
p nil
p 3.14
p [1, 2, 3]
p({ a: 1, b: 2 })
p true

# puts: array flattening + trailing-newline rules unchanged
puts "plain"
puts [1, 2, 3]
puts ["a\n", "b"]
puts

# print + to_s override
class T; def to_s; "T!"; end; end
print T.new, T.new
puts

# multi-arg p return values
p(p(1, 2))
x = p("single")
p x

# only inspect overridden — p uses it; (the native default object to_s
# carries a non-deterministic 0x address, so not exercised here)
class OnlyInspect; def inspect; "OI"; end; end
p OnlyInspect.new

# inspect that itself calls other methods (runs user code mid-p)
class Comp
  def initialize(n); @n = n; end
  def inspect; "Comp(#{double})"; end
  def double; @n * 2; end
end
p Comp.new(21)
