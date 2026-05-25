# Non-local return from a block — `return` inside `do…end` /
# `{ }` exits the enclosing method, not just the block. CRuby
# semantics. Counterpart case: `return` inside a regular `def`
# is local (just exits that method, doesn't escape its caller).

# Block-level return exits the enclosing method.
def find_first_even(arr)
  arr.each do |x|
    return x if x.even?
  end
  nil
end
puts find_first_even([1, 3, 5, 4, 7]).inspect    # 4
puts find_first_even([1, 3, 5]).inspect          # nil

# Helper method called from a block — its `return` is LOCAL
# (only exits the helper, the calling block continues).
def helper
  return "from helper"
end
def driver_local_return
  [1, 2].each { |x| puts helper }
  "after each"
end
puts driver_local_return
# Expected:
#   from helper
#   from helper
#   after each

# Nested blocks: deepest `return` still exits the enclosing method.
def find_in_2d(grid)
  grid.each do |row|
    row.each do |cell|
      return cell if cell > 10
    end
  end
  nil
end
p find_in_2d([[1, 2, 3], [4, 5, 99], [50]])      # 99
p find_in_2d([[1, 2], [3, 4]])                   # nil

# Short-circuit pattern: early return from inside .each_with_index.
def first_index_above(arr, threshold)
  arr.each_with_index do |v, i|
    return i if v > threshold
  end
  -1
end
puts first_index_above([10, 20, 30, 40], 25)     # 2
puts first_index_above([1, 2, 3], 100)           # -1

# Method-local return inside a method body — just exits that
# method, doesn't propagate further (control passes back to
# the caller).
def compute
  return 42
  999  # unreachable
end
puts compute                                     # 42
