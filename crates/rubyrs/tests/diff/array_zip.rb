# Array#zip — pair elements across one or more Arrays.

# Zero-arg: wraps each element in a 1-element Array.
puts [1, 2, 3].zip.inspect

# One arg, equal length.
puts [1, 2, 3].zip([4, 5, 6]).inspect

# Two args.
puts [1, 2, 3].zip([4, 5, 6], [7, 8, 9]).inspect

# Shorter argument — pads tail with nil.
puts [1, 2, 3].zip([4, 5]).inspect
puts [1, 2, 3].zip([10], [100, 200]).inspect

# Longer argument — truncated to receiver length.
puts [1, 2].zip([4, 5, 6]).inspect

# Empty receiver — always [].
puts [].zip([1, 2, 3]).inspect

# Empty argument — fills all rows with nil at that column.
puts [1, 2, 3].zip([]).inspect

# Mixed element types.
puts ["a", "b", "c"].zip([1, 2, 3]).inspect
puts [:x, :y, :z].zip([true, false, nil]).inspect

# Chain with map (the canonical use case — combine without an index).
result = [1, 2, 3].zip([4, 5, 6]).map { |pair| pair[0] + pair[1] }
puts result.inspect

# Chain with each.
sum = 0
[1, 2, 3].zip([10, 20, 30]).each do |pair|
  sum = sum + pair[0] * pair[1]
end
puts sum

# zip inside a class method.
class Pairing
  def initialize(xs, ys)
    @rows = xs.zip(ys)
  end
  def rows
    @rows
  end
end
p = Pairing.new([1, 2, 3], ["a", "b", "c"])
puts p.rows.inspect
puts p.rows.length

# Three-way zip with uneven lengths.
puts [1, 2, 3, 4].zip([10, 20], [100, 200, 300]).inspect

# Each row is a fresh Array (not aliased to inputs).
xs = [1, 2]
ys = [3, 4]
zipped = xs.zip(ys)
zipped[0][0] = 99
puts xs.inspect  # unchanged
puts zipped.inspect
