# Hash#merge — other's keys overwrite self's; new Hash returned
a = { x: 1, y: 2 }
b = { y: 20, z: 30 }
m = a.merge(b)
puts m.size
puts m[:x]
puts m[:y]   # overwritten by b
puts m[:z]
# original untouched
puts a.size
puts a[:y]
puts b[:y]

# Empty cases
puts({}.merge({}).size)
puts({ a: 1 }.merge({}).size)
puts({}.merge({ a: 1 }).size)

# Hash#to_a — array of [k, v] pairs
h = { a: 1, b: 2, c: 3 }
pairs = h.to_a
puts pairs.length
first = pairs[0]
puts first.length
puts first[0]
puts first[1]
last = pairs[2]
puts last[0]
puts last[1]

# Hash#to_h — identity
h2 = { foo: 1 }
puts h2.to_h.size
puts h2.to_h[:foo]

# Hash#delete — returns removed value, mutates
h3 = { a: 1, b: 2, c: 3 }
puts h3.delete(:b)
puts h3.size
puts h3[:b].nil?
puts h3.delete(:missing).nil?

# Hash#invert
inv = { a: 1, b: 2, c: 3 }.invert
puts inv.size
puts inv[1]
puts inv[2]
puts inv[3]
# Collisions: later value wins
collided = { a: 1, b: 1, c: 2 }.invert
puts collided.size       # 2 distinct values -> 2 entries
puts collided[1]         # b (CRuby keeps the LAST source key)
puts collided[2]         # c

# Hash#store — alias for []=
h4 = { x: 1 }
h4.store(:y, 2)
h4.store(:x, 99)   # overwrite existing
puts h4.size
puts h4[:x]
puts h4[:y]

# Hash#each_pair — alias for each, block sees (k, v)
out = []
{ a: 1, b: 2, c: 3 }.each_pair { |k, v| out << "#{k}=#{v}" }
puts out[0]
puts out[1]
puts out[2]

# Chained merge -> each
totals = { a: 1, b: 2 }
extra  = { b: 20, c: 30 }
sum = 0
totals.merge(extra).each { |_k, v| sum = sum + v }
puts sum   # 1 + 20 + 30 = 51

# Inside a class — a tiny "tally" pattern
class Tally
  def initialize
    @counts = {}
  end
  def bump(key)
    @counts.store(key, (@counts[key] || 0) + 1)
    self
  end
  def for(key)
    @counts[key] || 0
  end
  def keys_sorted
    @counts.keys.sort
  end
end

t = Tally.new
t.bump(:apple).bump(:apple).bump(:banana).bump(:apple)
puts t.for(:apple)
puts t.for(:banana)
puts t.for(:missing)
puts t.keys_sorted.length
puts t.keys_sorted[0]
puts t.keys_sorted[1]
