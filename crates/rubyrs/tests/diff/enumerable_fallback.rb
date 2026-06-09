# Array / Hash / Range reach Enumerable methods that have no native arm
# (minmax / minmax_by / each_entry / min(n) / max(n) / sum-with-block).
# Native iterators (sort/map/min/max/sum) still take precedence — no
# infinite recursion through Enumerable#sort.

# native still wins (regression guard)
p [3, 1, 2].sort
p [3, 1, 2].map { |x| x * 2 }
p [1, 2, 3].sum
p [1, 2, 3].min
p [1, 2, 3].max

# Array sum with block / non-numeric init
p [1, 2, 3].sum { |x| x * 2 }
p [1, 2, 3].sum(10) { |x| x * 2 }
p ["a", "b", "c"].sum("")
p [[1, 2], [3, 4]].sum([])

# Array minmax / minmax_by
p [3, 1, 4, 1, 5].minmax
p [3, 1, 2].minmax_by { |x| -x }

# Array min(n) / max(n), with and without comparator block
p [3, 1, 4, 1, 5].min(2)
p [3, 1, 4, 1, 5].max(2)
p [3, 1, 4, 1, 5].min(2) { |a, b| b <=> a }

# Array min/max with a comparator block (no n)
p [3, 1, 2].min { |a, b| b <=> a }
p [3, 1, 2].max { |a, b| b <=> a }

# Array each_entry
p [1, 2, 3].each_entry.to_a

# Range
p (1..5).minmax
p (1..5).min(2)
p (1..10).sum { |x| x }

# Hash (each yields [k, v] pairs)
p({ a: 1, b: 2 }.minmax)
p({ a: 1, b: 3, c: 2 }.max_by { |_, v| v })

# a user Enumerable class gets them too
class Bag
  include Enumerable
  def initialize(*a); @a = a; end
  def each(&b); @a.each(&b); end
end
b = Bag.new(3, 1, 2)
p b.minmax
p b.min(2)
p b.sum { |x| x * 10 }
p b.each_entry.to_a
