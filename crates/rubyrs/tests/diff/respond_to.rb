# Universal methods — every receiver answers true
puts 5.respond_to?(:nil?)
puts "x".respond_to?(:to_s)
puts nil.respond_to?(:nil?)
puts [].respond_to?(:==)

# Built-in type methods
puts 5.respond_to?(:abs)
puts 5.respond_to?(:times)
puts "abc".respond_to?(:length)
puts "abc".respond_to?(:upcase)
puts [1, 2].respond_to?(:push)
puts [1, 2].respond_to?(:each)
puts({ a: 1 }.respond_to?(:keys))
puts(:sym.respond_to?(:to_sym))
puts (1..5).respond_to?(:each)

# Negative — methods that don't exist on the receiver
puts 5.respond_to?(:upcase)
puts "abc".respond_to?(:abs)
puts [].respond_to?(:nonsense)

# String argument also accepted (CRuby allows both Sym and Str)
puts "abc".respond_to?("length")
puts 5.respond_to?("zero?")

# User-defined class: walks the class chain
class Animal
  def speak
    "generic"
  end
end
class Dog < Animal
  def bark
    "woof"
  end
end
a = Animal.new
d = Dog.new
puts a.respond_to?(:speak)
puts a.respond_to?(:bark)
puts d.respond_to?(:speak)   # inherited
puts d.respond_to?(:bark)
puts d.respond_to?(:fly)

# Feature-detection idiom — the actual use case
def label(x)
  if x.respond_to?(:upcase)
    x.upcase
  else
    x.to_s
  end
end
puts label("hello")
puts label(42)
