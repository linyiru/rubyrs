# Array#flat_map / each_slice / each_cons / chunk — the advanced
# Enumerable methods. CRuby's no-block forms return Enumerators;
# we directly return the Array of slices/windows so `.to_a`
# still works in the canonical idiom.

# flat_map — map then flatten depth 1.
p [[1, 2], [3, 4], [5]].flat_map { |a| a }
p [1, 2, 3].flat_map { |x| [x, x * 10] }
p [1, 2, 3].flat_map { |x| x }              # non-array element passes through
p [].flat_map { |x| [x] }

# Mixed: block can return both arrays and scalars.
p [1, 2, 3].flat_map { |n| n.even? ? [n, -n] : n }

# collect_concat — alias.
p [[:a, :b], [:c]].collect_concat { |a| a }

# each_slice — windowed by N, no overlap.
p [1, 2, 3, 4, 5].each_slice(2).to_a
p [1, 2, 3, 4, 5, 6].each_slice(3).to_a
p [1, 2, 3].each_slice(1).to_a
p [1, 2, 3].each_slice(10).to_a
p [].each_slice(2).to_a

# each_cons — windowed by N, sliding (overlap N-1).
p [1, 2, 3, 4, 5].each_cons(2).to_a
p [1, 2, 3, 4, 5].each_cons(3).to_a
p [1, 2, 3].each_cons(1).to_a
p [1, 2].each_cons(3).to_a    # too short: empty
p [].each_cons(2).to_a

# chunk — group consecutive elements by block return.
p [1, 1, 2, 2, 3, 3, 1].chunk { |x| x }.to_a
p [1, 2, 4, 9, 10, 11, 12, 15].chunk { |x| x.even? }.to_a
p [].chunk { |x| x }.to_a

# Chained chains.
sum_per_slice = [1, 2, 3, 4, 5, 6].each_slice(2).to_a.map { |s| s.sum }
p sum_per_slice

# flat_map for "expand each element".
expanded = (1..3).to_a.flat_map { |n| [n, n.to_s] }
p expanded

# each_cons for adjacent-pair-style iteration.
# Note: `|(a, b)|` block destructure isn't supported — use
# `|pair| pair[0]/pair[1]` instead, OR rely on block auto-splat
# (a, b will receive pair[0], pair[1] since pair is a 2-elem Array).
diffs = [10, 13, 15, 20, 26].each_cons(2).to_a.map { |a, b| b - a }
p diffs

# Inside a method.
class Sliced
  def initialize(arr)
    @arr = arr
  end
  def by(n)
    @arr.each_slice(n).to_a
  end
  def pairs
    @arr.each_cons(2).to_a
  end
end

s = Sliced.new([1, 2, 3, 4, 5])
p s.by(2)
p s.pairs

# Bad slice size.
begin
  [1, 2, 3].each_slice(0)
rescue ArgumentError
  puts "caught zero slice"
end

begin
  [1, 2, 3].each_cons(-1)
rescue ArgumentError
  puts "caught negative cons"
end
