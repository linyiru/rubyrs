# Op-assignment — `a OP= b` desugars to `a = a OP b`. Covers
# locals, ivars, Array/Hash index, and the short-circuit forms
# `||=` / `&&=`.

# Arithmetic on local.
a = 10
a += 5;  puts a
a -= 3;  puts a
a *= 2;  puts a
a /= 4;  puts a
a %= 4;  puts a

# Bit-ops on local.
b = 0b1100
b &= 0b1010; puts b
b |= 0b0001; puts b
b ^= 0b0011; puts b
b <<= 2;     puts b
b >>= 1;     puts b

# Ivar op-assign inside instance methods.
class Counter
  def initialize
    @n = 0
  end
  def inc
    @n += 1
  end
  def add(x)
    @n += x
  end
  def n; @n; end
end

c = Counter.new
c.inc
c.inc
c.add(5)
puts c.n

# Ivar arithmetic + bit ops together.
class Flags
  def initialize
    @bits = 0
  end
  def set(mask)
    @bits |= mask
  end
  def clear(mask)
    @bits &= ~mask
  end
  def toggle(mask)
    @bits ^= mask
  end
  def bits; @bits; end
end

f = Flags.new
f.set(0b0011)
puts f.bits
f.set(0b1000)
puts f.bits
f.clear(0b0010)
puts f.bits
f.toggle(0b1111)
puts f.bits

# Array index op-assign.
arr = [10, 20, 30, 40]
arr[0] += 1
puts arr.inspect
arr[2] -= 5
puts arr.inspect
arr[1] *= 3
puts arr.inspect
arr[-1] /= 2
puts arr.inspect

# Hash index op-assign — including auto-create-then-update idiom.
h = {"x" => 1, "y" => 2}
h["x"] += 10
puts h["x"]
h["new"] = 0
h["new"] += 5
puts h["new"]

# Counter pattern (common Hash idiom).
counts = {}
words = ["a", "b", "a", "c", "a", "b"]
words.each do |w|
  counts[w] ||= 0
  counts[w] += 1
end
puts counts["a"]
puts counts["b"]
puts counts["c"]
puts counts["missing"].inspect

# String concat via +=.
s = "Hello"
s += ", World"
puts s

# Float op-assign.
f = 1.0
f += 0.5
puts f
f *= 2
puts f
f -= 0.25
puts f

# ||= and &&= on locals.
x = nil
x ||= 5
puts x
x ||= 99
puts x

y = false
y ||= "set"
puts y

z = 10
z &&= 20
puts z

w = nil
w &&= "wont-assign"
puts w.inspect

# ||= on a fresh local works (declares + assigns when nil).
fresh ||= "first"
puts fresh
fresh ||= "second"
puts fresh

# ||= and &&= on ivars.
class Cache
  def get(k)
    @store ||= {}
    @store[k] ||= "default-#{k}"
  end
  def store
    @store
  end
end

cache = Cache.new
puts cache.get("alpha")
puts cache.get("beta")
puts cache.get("alpha")
puts cache.store.length

# Op-assign inside an iterator (closure-style use).
sum = 0
[1, 2, 3, 4, 5].each { |n| sum += n }
puts sum

product = 1
[1, 2, 3, 4].each { |n| product *= n }
puts product

# Op-assign with a method-call RHS.
class Bag
  def initialize
    @items = []
  end
  def add(x)
    @items += [x]
  end
  def items; @items; end
end
b = Bag.new
b.add(1); b.add(2); b.add(3)
puts b.items.inspect

# Array#concat via +=.
xs = [1, 2]
xs += [3, 4]
puts xs.inspect

# Chained / nested op-assigns.
class Stats
  def initialize
    @sum = 0
    @count = 0
  end
  def push(x)
    @sum += x
    @count += 1
  end
  def avg
    return 0 if @count == 0
    @sum / @count
  end
end

s = Stats.new
[10, 20, 30, 40].each { |n| s.push(n) }
puts s.avg
