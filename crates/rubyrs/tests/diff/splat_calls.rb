# Splat at the call site (`foo(*arr)`) and rest-param in method
# defs (`def vsum(*nums)`). Currently only single-arg splat
# call shape; mixed splats like `foo(a, *b, c)` aren't supported.

# Splat call expanding into positional params.
def foo(a, b, c)
  "#{a}-#{b}-#{c}"
end

p foo(*[1, 2, 3])
p foo(*[10, 20, 30])

# Stored Array.
args = [4, 5, 6]
p foo(*args)

# Rest-param gathers all positional args.
def vsum(*nums)
  total = 0
  nums.each { |n| total += n }
  total
end

p vsum(1, 2, 3, 4)
p vsum                  # zero args → empty rest
p vsum(*[10, 20, 30])   # splat into rest
p vsum(*[])

# Rest after required positional args.
def mix(first, *rest)
  "#{first}: #{rest.inspect}"
end
p mix("a", "b", "c", "d")
p mix("alone")
p mix(*["x", "y", "z"])

# Splat-call into a method with rest param.
def collector(*items)
  items
end
arr = [1, 2, 3, 4, 5]
p collector(*arr)
p collector(*[])

# Class methods.
class Bag
  def initialize(*items)
    @items = items
  end
  def items
    @items
  end
  def push_all(*more)
    @items += more
    @items
  end
end

b = Bag.new(1, 2, 3)
p b.items
b.push_all(4, 5)
p b.items
b.push_all(*[6, 7, 8])
p b.items

# Splat through a method chain.
def double_all(arr)
  arr.map { |x| x * 2 }
end

vals = [5, 10, 15]
p vsum(*double_all(vals))

# Rest with defaults: not currently supported in our subset
# (rest precludes optional positional defaults), but rest with
# explicit required prefix works.
def header(label, *cols)
  "#{label}=[#{cols.join(',')}]"
end
puts header("row", "a", "b", "c")
puts header("alone")
puts header(*["fan", "1", "2"])

# Apply-style: split into name + rest using rest-param.
def split_off(first, *rest)
  [first, rest]
end
p split_off(*[10, 20, 30, 40])
