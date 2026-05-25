# Array#bsearch — binary search a sorted Array with a block.
# Two modes determined by the block's return type:
#   find-minimum (Bool): smallest element where block is truthy.
#   find-any (Int): 0 = match, <0 = "x too large", >0 = "x too small".

# find-minimum mode (Bool return).
puts [1, 3, 5, 7, 9].bsearch { |x| x >= 5 }                  # 5
puts [1, 3, 5, 7, 9].bsearch { |x| x >= 4 }                  # 5
puts [1, 3, 5, 7, 9].bsearch { |x| x >= 100 }.inspect        # nil
puts [1, 3, 5, 7, 9].bsearch { |x| x >= 1 }                  # 1

# find-any mode (Int return).
puts [1, 3, 5, 7, 9].bsearch { |x| 5 - x }                   # 5
puts [1, 3, 5, 7, 9].bsearch { |x| 1 - x }                   # 1
puts [1, 3, 5, 7, 9].bsearch { |x| 9 - x }                   # 9
puts [1, 3, 5, 7, 9].bsearch { |x| 6 - x }.inspect           # nil
puts [1, 3, 5, 7, 9].bsearch { |x| 0 - x }.inspect           # nil

# Empty array.
puts [].bsearch { |x| true }.inspect                         # nil
puts [].bsearch { |x| 1 - x }.inspect                        # nil

# Single element.
puts [42].bsearch { |x| x >= 42 }                            # 42
puts [42].bsearch { |x| x >= 50 }.inspect                    # nil

# Large sorted array — log-scale lookup.
xs = (1..10000).step(2).to_a  # [1, 3, 5, ..., 9999]
puts xs.bsearch { |x| x >= 5001 }                            # 5001
puts xs.bsearch { |x| x >= 9999 }                            # 9999
puts xs.bsearch { |x| x >= 10000 }.inspect                   # nil
