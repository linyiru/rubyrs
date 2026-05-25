# Basic — assign two locals from an Array
a, b = [10, 20]
puts a
puts b

# Three from a three-element Array
x, y, z = [1, 2, 3]
puts x
puts y
puts z

# More targets than elements — extras get nil
c, d, e = [1, 2]
puts c
puts d
puts e.nil?

# Fewer targets — extras are silently dropped
p, q = [100, 200, 300]
puts p
puts q

# RHS as comma-separated values (no explicit `[ ]`) — Prism
# packs into an ArrayNode at the value slot
r, s = 1, 2
puts r
puts s

t, u, v = "a", "b", "c"
puts t
puts u
puts v

# From a method that returns an Array
def pair
  [11, 22]
end
m, n = pair
puts m
puts n

# Combined with Array#partition — the original motivating idiom
evens, odds = [1, 2, 3, 4, 5, 6].partition { |n| n.even? }
puts evens.length
puts odds.length
puts evens[0]
puts evens[2]
puts odds[0]

# IVar destructuring inside a class initializer
class Point
  attr_reader :x, :y
  def initialize(coords)
    @x, @y = coords
  end
end
p = Point.new([3, 7])
puts p.x
puts p.y

class Range3D
  attr_reader :a, :b, :c
  def initialize
    @a, @b, @c = [1, 2, 3]
  end
end
r = Range3D.new
puts r.a
puts r.b
puts r.c

# Mixed locals + ivars (separate statements; we don't support
# `@x, y = ...` mixing in one assignment yet, but each form
# works independently)

# Reassignment overwrites
n = 99
m = 99
n, m = [7, 8]
puts n
puts m

# Inside a method that returns a 2-element Array
def split(arr)
  if arr.length == 2
    arr
  else
    [0, 0]
  end
end
first, second = split([42, 43])
puts first
puts second
first2, second2 = split([])
puts first2
puts second2

# Multi-write inside a method body that returns
def stats(nums)
  evens, odds = nums.partition { |n| n.even? }
  "evens=#{evens.length} odds=#{odds.length}"
end
puts stats([1, 2, 3, 4, 5])
puts stats([])

# Chained: assign result of method that calls another method
def coordinates
  [10, 20, 30]
end
lat, lon = coordinates
puts lat
puts lon
