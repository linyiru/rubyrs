# Single optional param, default takes effect
def greet(name = "world")
  puts "hello, #{name}"
end
greet
greet "ruby"

# Mixed required + optional
def add(x, y = 1)
  x + y
end
puts add(5)
puts add(5, 10)

# Multiple optionals, can omit some or all
def make(a = 1, b = 2, c = 3)
  puts a
  puts b
  puts c
  puts "---"
end
make
make 10
make 10, 20
make 10, 20, 30

# Default = nil — common Gemfile idiom
def open(path, mode = nil)
  if mode.nil?
    "open #{path}"
  else
    "open #{path} (#{mode})"
  end
end
puts open("a.txt")
puts open("b.txt", "r")

# Default = boolean literal
def flag(name, on = true)
  if on
    "#{name}: on"
  else
    "#{name}: off"
  end
end
puts flag("alpha")
puts flag("beta", false)

# Inside a class, plus method chained
class Greeter
  def initialize(name = "anon")
    @name = name
  end
  def hello(prefix = "hi")
    "#{prefix}, #{@name}"
  end
end
puts Greeter.new.hello
puts Greeter.new("ruby").hello
puts Greeter.new("rs").hello("hey")
