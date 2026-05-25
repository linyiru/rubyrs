# Enumerable#min_by(n) / #max_by(n) — top-n form.
# min_by(n) returns the n smallest by key, sorted ascending.
# max_by(n) returns the n largest by key, sorted descending.
# Edges: n=0 → []; n > len → all elements sorted; n<0 → ArgumentError.

xs = [3, 1, 4, 1, 5, 9, 2, 6, 5, 3]

puts xs.min_by(3) { |x| x }.inspect          # [1, 1, 2]
puts xs.max_by(3) { |x| x }.inspect          # [9, 6, 5]
puts xs.min_by(1) { |x| x }.inspect          # [1]
puts xs.min_by(0) { |x| x }.inspect          # []

# n exceeds length — all elements, sorted.
puts [1, 3, 2].max_by(10) { |x| x }.inspect  # [3, 2, 1]
puts [1, 3, 2].min_by(10) { |x| x }.inspect  # [1, 2, 3]

# Empty input.
puts [].min_by(3) { |x| x }.inspect          # []
puts [].max_by(3) { |x| x }.inspect          # []

# Non-trivial key — sort by absolute distance from 5.
nums = [1, 4, 8, 5, 12, 3]
puts nums.min_by(3) { |x| (x - 5).abs }.inspect  # [5, 4, 3]
puts nums.max_by(2) { |x| (x - 5).abs }.inspect  # [12, 1]

# String keys.
words = ["apple", "pear", "kiwi", "banana"]
puts words.min_by(2) { |w| w.length }.inspect    # ["pear","kiwi"]
puts words.max_by(2) { |w| w.length }.inspect    # ["banana","apple"]

# Negative n raises.
begin
  [1, 2, 3].min_by(-1) { |x| x }
rescue ArgumentError => e
  puts "negative: caught"
end
