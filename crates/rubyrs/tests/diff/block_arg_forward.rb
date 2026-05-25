# Block-argument forwarding: `&proc_value` passes an existing
# Proc / lambda as the block to a block-taking method. Closes
# the F1 deferred sub-feature.

# Basic forward of a lambda into Array#map.
double = lambda { |x| x * 2 }
puts [1, 2, 3].map(&double).inspect

# Same with proc { ... }.
square = proc { |x| x * x }
puts [1, 2, 3, 4].map(&square).inspect

# Forward into each.
greeter = lambda { |name| puts "hi, #{name}" }
%w[a b c].each(&greeter)

# Forward into reject / select.
positive = lambda { |x| x > 0 }
puts [-2, -1, 0, 1, 2].select(&positive).inspect
puts [-2, -1, 0, 1, 2].reject(&positive).inspect

# Chaining: store a series of transforms.
inc = lambda { |x| x + 1 }
puts [10, 20].map(&inc).map(&inc).inspect       # [12, 22]

# Coexists with literal blocks elsewhere on the line.
sum = 0
[1, 2, 3].each { |x| sum = sum + x }
adder = lambda { |x| sum = sum + x }
[10, 20].each(&adder)
puts sum                                         # 6 + 30 = 36

# Forward into inject/reduce.
add = lambda { |acc, x| acc + x }
puts [1, 2, 3, 4].inject(0, &add)                # 10

# Symbol-to-proc still works (different parse path).
puts [1, -2, 3].map(&:abs).inspect               # [1, 2, 3]
