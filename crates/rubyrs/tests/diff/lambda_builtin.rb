# `lambda { ... }` and `proc { ... }` block-to-Proc capture. Both
# return the attached block as a Proc-shaped Value the script can
# call later. rubyrs doesn't distinguish Lambda from Proc at runtime
# (the strict-arity check is the documented gap in SUBSET.md), so
# both names produce the same thing.

# Basic lambda with .call / .() / [] invocation syntaxes.
l = lambda { |x| x * 2 }
puts l.call(5)
puts l.(7)
puts l[3]

# Lambda capturing outer scope.
n = 100
add_n = lambda { |x| x + n }
puts add_n.call(5)
puts add_n.(20)

# Stored on an instance variable + called from a method.
class Pipeline
  def initialize
    @stages = []
  end
  def stage(fn)
    @stages << fn
    self
  end
  def apply(x)
    @stages.each { |fn| x = fn.call(x) }
    x
  end
end

p2 = Pipeline.new
p2.stage(lambda { |x| x + 1 })
p2.stage(lambda { |x| x * 10 })
puts p2.apply(5)                 # (5+1)*10 = 60

# `proc { ... }` — same shape; rubyrs doesn't enforce Lambda strict
# arity, so `proc` and `lambda` look identical here.
p = proc { |a, b| a - b }
puts p.call(10, 3)
puts p.(20, 5)

# Capture-by-name: store a lambda, hand it back through helpers,
# call it indirectly. Forwarding `&lam` into a block-taking method
# (`arr.map(&double)`) is a separate feature still on the roadmap.
double = lambda { |x| x * 2 }
puts double.call(7)              # 14
puts double.call(double.call(5)) # 20
