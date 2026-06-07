# Explicit-receiver dispatch correctness across the monomorphic inline
# cache fast path: polymorphism, args, inheritance, private (fall-through
# to NoMethodError), send-bypass, method_missing, singleton shadowing,
# and non-fixed arity.
class Animal
  def initialize(n)
    @n = n
  end
  def speak
    "..."
  end
  def describe(prefix)
    "#{prefix}:#{speak}:#{@n}"
  end
  private
  def secret
    "hidden"
  end
end
class Dog < Animal
  def speak
    "woof"
  end
end
class Cat < Animal
  def speak
    "meow"
  end
end

animals = [Dog.new(1), Cat.new(2), Dog.new(3)]
p animals.map(&:speak)                       # polymorphic call site
p animals.map { |a| a.describe("hi") }       # inherited method + ivar + arg

# Private method with an explicit receiver → NoMethodError; send bypasses.
begin
  Dog.new(1).secret
rescue NoMethodError => e
  puts "private_caught:#{e.message.include?('private')}"
end
p Dog.new(1).send(:secret)

# method_missing fall-through (lookup returns None → slow path).
class Ghost
  def method_missing(name, *args)
    "mm:#{name}:#{args.inspect}"
  end
end
p Ghost.new.boo(1, 2)

# Singleton method shadows the class method, at the SAME call site.
d = Dog.new(9)
def d.speak
  "SINGLETON"
end
out = []
[d, Dog.new(9)].each { |x| out << x.speak }
p out                                         # ["SINGLETON", "woof"]

# Args + non-fixed arity (default) still correct.
class Calc
  def add(a, b)
    a + b
  end
  def opt(a, b = 10)
    a + b
  end
end
c = Calc.new
p [c.add(3, 4), c.opt(5), c.opt(5, 100)]
sum = 0
1000.times { sum += c.add(1, 1) }            # warm the cache
p sum
