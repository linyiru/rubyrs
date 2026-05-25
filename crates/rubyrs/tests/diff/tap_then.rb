# Object#tap / #then / #yield_self — universal block helpers.
#   tap: yield self, discard block return, return self.
#   then / yield_self: yield self, return block result.

# Basic forms — every Value type.
p 1.tap { |n| puts "saw #{n}" }
p "hi".tap { |s| puts s.upcase }
p :sym.tap { |s| puts s.to_s }
p [1, 2].tap { |a| puts a.length }
p({a: 1}.tap { |h| puts h.length })
p nil.tap { puts "nil too" }
p true.tap { |b| puts "bool: #{b}" }

# then / yield_self return whatever the block returns.
p 5.then { |x| x * 2 }
p 5.yield_self { |x| x + 100 }
p "hello".then { |s| s.upcase }
p [1, 2, 3].then { |a| a.sum }

# Chained tap for debug breadcrumbs.
result = [1, 2, 3]
  .tap { |a| puts "before: #{a.length}" }
  .map { |x| x * 2 }
  .tap { |a| puts "after: #{a.inspect}" }
  .sum
puts result

# Chained then for transforms.
out = "  hello  "
  .then { |s| s.strip }
  .then { |s| s.upcase }
  .then { |s| "[#{s}]" }
puts out

# tap with side-effect mutation.
arr = [1, 2, 3]
arr.tap { |a| a << 4 }
p arr

# then for nil-safety isn't quite the Ruby `&.` style but is
# useful as a one-shot pipeline kick-off.
p 0.then { |n| n.zero? ? "zero" : "non" }

# Inside a method.
class Builder
  def initialize
    @parts = []
  end
  def add(x)
    self.tap { |b| b.instance_variable_get(:@parts) << x }
  end
  def result
    @parts
  end
end

# instance_variable_get isn't implemented; substitute a simple
# add via attr access.
class Builder2
  attr_reader :parts
  def initialize
    @parts = []
  end
  def add(x)
    @parts << x
    self
  end
end

b = Builder2.new.add(1).add(2).tap { |x| puts "built #{x.parts.length}" }.add(3)
p b.parts

# yield_self around a method-call result.
def double(x); x * 2; end
result = double(7).yield_self { |n| n + 1 }
puts result
