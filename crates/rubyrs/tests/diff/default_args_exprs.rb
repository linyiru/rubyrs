# Default values for positional params allowed to be any expression.
# rubyrs used to require literals (Int/Str/Sym/true/false/nil).
# Both rubyrs and CRuby must produce identical stdout.

# Literal still works
def f1(a, b = 42); [a, b]; end
puts f1(1).inspect
puts f1(1, 2).inspect

# Constant reference (the rake/linked_list shape)
EMPTY = []
def f2(a, b = EMPTY); [a, b]; end
puts f2(:x).inspect
puts f2(:x, [9]).inspect

# Reference to an earlier positional param
def f3(a, b = a + 1, c = a + b); [a, b, c]; end
puts f3(10).inspect
puts f3(10, 100).inspect
puts f3(10, 100, 999).inspect

# Method call as default
def make_default; "from method"; end
def f4(x = make_default); x; end
puts f4
puts f4("override")

# Arithmetic + frozen string
def f5(label = "v#{1 + 2}".freeze); label; end
puts f5
puts f5("custom")

# Inside a class — default referencing a class-method
class Box
  def self.unit; 7; end
  def initialize(size = Box.unit); @size = size; end
  attr_reader :size
end
puts Box.new.size
puts Box.new(99).size

# Default evaluated each call (not cached) — CRuby re-evaluates.
# Use an Array as the shared counter cell (globals aren't supported).
COUNTER = [0]
def bump; COUNTER[0] = COUNTER[0] + 1; COUNTER[0]; end
def f6(n = bump); n; end
puts f6  # 1
puts f6  # 2
puts f6(100)  # 100 — default NOT evaluated when arg supplied
puts f6  # 3 — back to bump
