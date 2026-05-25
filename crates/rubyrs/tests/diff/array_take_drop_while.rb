# Array#take_while / #drop_while — prefix partition.
# take_while: prefix of elements where the block stays truthy.
# drop_while: skip that prefix, return the rest.
# Block is invoked only until the first falsy return.

# Basic.
puts [1, 2, 3, 4, 5, 2, 1].take_while { |x| x < 4 }.inspect
# → [1, 2, 3]
puts [1, 2, 3, 4, 5, 2, 1].drop_while { |x| x < 4 }.inspect
# → [4, 5, 2, 1] (note: includes elements that would be truthy
#                later — drop_while stops checking at crossing)

# Block truthy for everything.
puts [1, 2, 3].take_while { |x| true }.inspect      # [1, 2, 3]
puts [1, 2, 3].drop_while { |x| true }.inspect      # []

# Block immediately false.
puts [5, 6, 7].take_while { |x| x < 1 }.inspect     # []
puts [5, 6, 7].drop_while { |x| x < 1 }.inspect     # [5, 6, 7]

# Empty receiver.
puts [].take_while { |x| true }.inspect             # []
puts [].drop_while { |x| true }.inspect             # []

# Real-world idiom: strip leading zeros / headers.
puts [0, 0, 0, 1, 2, 0, 3].drop_while { |x| x == 0 }.inspect
# → [1, 2, 0, 3]

# String-element example.
words = ["alpha", "beta", "GO", "gamma", "delta"]
puts words.take_while { |w| w == w.downcase }.inspect
# → ["alpha", "beta"]
puts words.drop_while { |w| w == w.downcase }.inspect
# → ["GO", "gamma", "delta"]

# Non-mutating.
xs = [1, 2, 3, 4]
xs.take_while { |x| x < 3 }
puts xs.inspect                                      # [1, 2, 3, 4]
