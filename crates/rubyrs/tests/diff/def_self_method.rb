# `def self.method` — singleton method definitions on classes.
# Covers the basic shape plus a few variations: with-args, with-defaults,
# multiple per class, mixed with instance methods, and inherited classes.

class Animal
  def self.kingdom
    "Animalia"
  end

  def self.greet(name)
    "hello, #{name}"
  end

  # Default-arg case for symmetry with regular def — confirms the kw /
  # rest / default plumbing still works on the singleton compile path.
  def self.tag(name, suffix = "!")
    "#{name}#{suffix}"
  end

  # Instance method on the same class — should NOT be reachable from
  # `Animal.legs`, only from an instance.
  def legs
    4
  end
end

puts Animal.kingdom                  # Animalia
puts Animal.greet("Rex")             # hello, Rex
puts Animal.tag("Mochi")             # Mochi!
puts Animal.tag("Mochi", " (cat)")   # Mochi (cat)

# Verify instance methods don't leak onto the class.
begin
  Animal.legs
  puts "FAIL: instance method visible on class"
rescue NoMethodError => e
  puts "ok: NoMethodError for instance call on class"
end

# Singleton on subclass — independent table per class.
class Dog < Animal
  def self.breed_count
    100
  end
end

puts Dog.breed_count                 # 100
# CRuby walks the singleton-class chain along the superclass spine,
# so Dog inherits Animal's class-method.
puts Dog.kingdom                     # Animalia

# Reopening — second `def self.x` overrides the first.
class Animal
  def self.kingdom
    "Animalia (override)"
  end
end
puts Animal.kingdom                  # Animalia (override)
