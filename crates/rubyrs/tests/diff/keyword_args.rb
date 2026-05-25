# Keyword arguments — `def foo(name:, age: 0)` + `foo(name: "x")`
# at the call site. The trailing `name: value, ...` group becomes
# a Hash arg the callee splits into named bindings; required
# keywords missing from the call raise ArgumentError.

def greet(name:, greeting: "hi")
  "#{greeting}, #{name}"
end

puts greet(name: "alice")
puts greet(name: "bob", greeting: "hello")

# Order doesn't matter for keyword args.
puts greet(greeting: "hey", name: "carol")

# Missing required keyword raises ArgumentError.
begin
  greet(greeting: "ola")
rescue ArgumentError => e
  puts "missing: #{e.message}"
end

begin
  greet
rescue ArgumentError => e
  puts "empty: #{e.message}"
end

# All defaults — no required kw.
def make(width: 80, height: 24)
  "#{width}x#{height}"
end
puts make
puts make(width: 100)
puts make(height: 50)
puts make(width: 200, height: 100)

# Mix positional + keyword.
def slot(pos, kw:)
  "#{pos}/#{kw}"
end
puts slot("a", kw: "b")
puts slot("a", kw: "z")

# Multiple positionals + multiple keywords.
def report(label, count, level: :info, prefix: "")
  "#{prefix}[#{level}] #{label}=#{count}"
end
puts report("hits", 42)
puts report("hits", 42, level: :warn)
puts report("hits", 42, prefix: ">> ", level: :error)

# Inside a class method.
class Person
  def initialize(name:, age: 0)
    @name = name
    @age = age
  end
  attr_reader :name, :age
end

p1 = Person.new(name: "alice", age: 30)
puts "#{p1.name}/#{p1.age}"
p2 = Person.new(name: "bob")
puts "#{p2.name}/#{p2.age}"

# Keyword with literal default values of various types.
def show(s: "default", n: 1, f: 1.5, b: true, x: nil)
  "#{s}|#{n}|#{f}|#{b}|#{x.inspect}"
end
puts show
puts show(s: "X")
puts show(b: false, x: 42)

# All required keywords.
def vector(x:, y:, z:)
  [x, y, z]
end
p vector(x: 1, y: 2, z: 3)
p vector(z: 30, y: 20, x: 10)
