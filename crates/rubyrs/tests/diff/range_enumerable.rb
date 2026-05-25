# Range Enumerable — the methods now derived from materializing
# an Int-bounded Range as an Array and dispatching through
# Array's iteration arms. Covers each_with_index,
# each_with_object, partition, min_by, max_by, group_by,
# sort_by, plus the no-block `sort`.

# sort with no block — Range is already ascending.
puts (1..5).sort.inspect
puts (1...5).sort.inspect
puts (5..1).sort.inspect      # empty range -> []
puts (3..3).sort.inspect      # single-element

# each_with_index — yields (value, index).
(1..5).each_with_index do |v, i|
  puts "#{i}:#{v}"
end

# Exclusive endpoint.
(0...4).each_with_index do |v, i|
  puts "ex #{i}=#{v}"
end

# partition splits into [matching, non-matching].
puts (1..10).partition { |x| x.even? }.inspect
puts (1..6).partition { |x| x > 3 }.inspect
puts (1..1).partition { |x| true }.inspect

# min_by / max_by use the block as a key projection.
puts (1..5).min_by { |x| (x - 3).abs }
puts (1..5).max_by { |x| (x - 3).abs }
puts (1..10).min_by { |x| -x }
puts (1..10).max_by { |x| -x }

# group_by buckets by the block's return.
puts (1..10).group_by { |x| x % 3 }.inspect
puts (1..6).group_by { |x| x < 4 ? "low" : "high" }.inspect

# sort_by orders by the block's return.
puts (1..5).sort_by { |x| -x }.inspect
puts (1..6).sort_by { |x| x % 3 }.inspect    # stable order within group

# each_with_object threads a memo Array.
result = (1..4).each_with_object([]) { |x, memo| memo << x * x }
puts result.inspect

# Counter idiom via each_with_object Hash.
counts = (1..10).each_with_object({}) do |n, h|
  bucket = n.even? ? :even : :odd
  h[bucket] ||= 0
  h[bucket] += 1
end
puts counts.inspect

# Chains: Range -> sort_by -> first.
top3 = (1..20).sort_by { |x| -(x * x) }.take(3)
puts top3.inspect

# Range -> partition -> .map.
evens, odds = (1..10).partition { |x| x.even? }
puts evens.map { |x| x * 2 }.inspect
puts odds.map { |x| x + 100 }.inspect

# break inside Range#each_with_index works (general iterator
# protocol — not specific to Range, but confirms the
# materialize-and-delegate path doesn't break it).
result = (1..10).each_with_index do |v, i|
  break "hit #{v}@#{i}" if v == 5
end
puts result

# Inside a method.
class Histogram
  def initialize(rng)
    @buckets = rng.group_by { |n| n / 10 }
  end
  def keys
    @buckets.keys.sort
  end
  def counts
    @buckets.keys.sort.map { |k| @buckets[k].length }
  end
end

h = Histogram.new(1..30)
puts h.keys.inspect
puts h.counts.inspect
