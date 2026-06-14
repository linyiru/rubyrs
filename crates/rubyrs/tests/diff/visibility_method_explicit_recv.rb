# public / private / protected called as a METHOD on an explicit Class
# receiver (e.g. `Klass.send(:public, :m)` or via singleton_class) flips
# the named instance method's visibility. rack's request spec does
# `req.singleton_class.send(:public, :forwarded_scheme)`.

class Foo
  def a; "a"; end
  def b; "b"; end
  private :a, :b
end

f = Foo.new
Foo.send(:public, :a)
p f.a                       # "a" (re-publicised)
begin
  f.b
  puts "b callable"
rescue NoMethodError
  puts "b still private"
end

# return value: single arg -> the symbol, multi -> array
p Foo.send(:public, :b)     # :b
Bar = Class.new do
  def x; end
  def y; end
end
p Bar.send(:private, :x, :y).sort   # [:x, :y]

# on a singleton class (re-expose a singleton method)
obj = Object.new
def obj.secret; "s"; end
obj.singleton_class.send(:private, :secret)
obj.singleton_class.send(:public, :secret)
p obj.secret                # "s"
