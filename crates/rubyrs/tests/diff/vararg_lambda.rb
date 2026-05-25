# Vararg lambda / proc — `|*args|` and `|head, *rest|`.

# Pure splat.
l = lambda { |*args| args.inspect }
puts l.call(1, 2, 3)
puts l.call
puts l.(10)

# Leading required + splat.
p = proc { |a, *rest| "#{a} / #{rest.inspect}" }
puts p.call(1, 2, 3, 4)
puts p.call(99)            # rest is []
puts p.call                # a is nil, rest is []

# Arrow-style lambda with splat.
sum = ->(*nums) { nums.inject(0) { |acc, n| acc + n } }
puts sum.call(1, 2, 3, 4, 5)
puts sum.call

# Capture from outer scope + splat.
multiplier = 10
scale = lambda { |*xs| xs.map { |x| x * multiplier }.inspect }
puts scale.call(1, 2, 3)

# Forward splat into call args.
def collect(*items); items.inspect; end
forward = lambda { |*args| collect(*args) }
puts forward.call("a", "b", "c")

# Mixed: positional + rest, applied via splat call.
nums = [10, 20, 30, 40]
puts p.call(*nums)         # 10 / [20, 30, 40]

# Inside a class — store + call later.
class Acc
  def initialize
    @items = []
    @adder = lambda { |*xs| @items = @items + xs }
  end
  def add(*xs); @adder.call(*xs); end
  def items; @items; end
end
a = Acc.new
a.add(1, 2)
a.add(3, 4, 5)
puts a.items.inspect
