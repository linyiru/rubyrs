# class of built-in types — returns the Class object, .name a String
puts 5.class
puts 5.class.name
puts "x".class.name
puts :sym.class.name
puts [].class.name
puts({}.class.name)
puts (1..3).class.name
puts true.class.name
puts false.class.name
puts nil.class.name

# Class equality is identity — same class returns true,
# different class returns false. Most idiomatic use of `.class`.
class Animal
end
class Dog < Animal
end

a = Animal.new
d = Dog.new
puts a.class == Animal
puts d.class == Dog
puts d.class == Animal      # false — exact class, not is_a
puts a.class != Dog

# Class.name on a user class
puts a.class.name
puts d.class.name

# class.to_s same as name for Class
puts Animal.to_s
puts Dog.to_s

# The actual idiom: comparing class name as String
e = Dog.new
puts e.class.name == "Dog"
puts e.class.name != "Animal"

# class.class — meta level. Every class's class is `Class`.
puts Animal.class
puts Animal.class.name

# Inside a rescue handler — class-name dispatch
class MyError < StandardError
end
begin
  raise MyError, "boom"
rescue => err
  puts err.class.name
  puts err.class == MyError
end
