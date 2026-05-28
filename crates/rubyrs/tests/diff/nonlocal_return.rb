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

## Layer #4 shapes: lexical-owner unwind. `return` exits the
## method that LEXICALLY DEFINED the block, not the
## dynamic-context method that's currently yielding. Pre-fix
## rubyrs unwound "while is_block" and so stopped at the
## nearest non-block frame (= the yielder), leaving the
## lexical-owner method running with the wrong value.
## (TRY_RUNS pass-10 layer #4.)

## L4-Shape 1: `return` from a block defined in caller_method
## but YIELDED by `outer`. CRuby: exits caller_method →
## caller_method's caller sees nothing further. Pre-fix:
## exited `outer` only, so caller_method got `r = :b` and
## printed "after: :b". Post-fix: caller_method itself returns,
## printing the block's value `:b` directly.
def yield_outer(items)
  items.each { |it| yield it }
  "yielder-fell-through"
end
def lexical_owner_1
  r = yield_outer([:a, :b, :c]) do |x|
    return x if x == :b
  end
  "after: #{r.inspect}"
end
puts "L4-1=#{lexical_owner_1}"

## L4-Shape 2: nested-block — `return` from an inner block
## whose lexical owner is `triple`. CRuby unwinds through both
## block frames AND any methods between them.
def triple
  [1].each do
    [2].each do
      return "got-out"
    end
    "inner-fell"
  end
  "outer-fell"
end
puts "L4-2=#{triple}"
