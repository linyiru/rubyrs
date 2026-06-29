# ADR 0034 Step 1 (pieces 1+2) — an Object-param method that obtains an Array from an
# obj-call getter (`arr = node.values`) and iterates it by index (`arr.length`,
# `arr[i]`) goes native. A step toward the rubocop AST-walk trunk (`kids = node.children;
# kids[i]`). Parity must hold interpreter == JIT == CRuby, including deopts.

class Node
  def initialize(vals); @vals = vals; end
  def values; @vals; end                 # getter returning an Int Array
end

def sumvals(node)                         # Object param
  arr = node.values                       # obj-call -> Array (jit_obj_getter_array)
  s = 0
  i = 0
  n = arr.length                          # local-array length
  while i < n
    s += arr[i]                           # local-array index (Int element)
    i += 1
  end
  s
end

p sumvals(Node.new((0...10).to_a))        # 45
p sumvals(Node.new([100, 200, 300]))      # 600
p sumvals(Node.new([]))                   # 0 (empty array)

# Polymorphic receiver class (different `values` getter) must deopt + stay correct.
class Node2 < Node
  def values; @vals.map { |x| x * 2 }; end
end
p sumvals(Node2.new([1, 2, 3]))           # (2+4+6) = 12

# The ivar is not an Array (Int) -> the getter-array read deopts; CRuby raises
# NoMethodError on `.length` for an Integer. Only stdout is compared, so guard it.
class Bad
  def initialize; @vals = 7; end
  def values; @vals; end
end
begin
  sumvals(Bad.new)
  p :no_raise
rescue NoMethodError
  p :nomethod
end

# A non-Int element in the array -> the element read deopts to the interpreter.
class Mixed
  def initialize; @vals = [1, "two", 3]; end
  def values; @vals; end
end
begin
  p sumvals(Mixed.new)
rescue => e
  p e.class.name
end
