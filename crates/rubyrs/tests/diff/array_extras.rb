# reverse — returns a new Array, source untouched
src = [1, 2, 3, 4]
rev = src.reverse
puts rev[0]
puts rev[3]
puts src[0]
puts src[3]

# uniq — preserves first-seen order, uses == for equality
puts [1, 2, 2, 3, 1, 4].uniq.length
arr = [1, 2, 2, 3, 1, 4].uniq
puts arr[0]
puts arr[1]
puts arr[2]
puts arr[3]
puts ["a", "b", "a", "c"].uniq.length
puts [].uniq.length

# compact — drops nils
puts [1, nil, 2, nil, 3].compact.length
puts [nil, nil].compact.length
puts [1, 2, 3].compact.length

# flatten — shallow flatten (depth 1)
nested = [[1, 2], [3, 4], [5]]
flat = nested.flatten
puts flat.length
puts flat[0]
puts flat[4]
mixed = [1, [2, 3], 4, [5]]
fm = mixed.flatten
puts fm.length
puts fm[1]
puts fm[3]

# join — default separator is empty string
puts [1, 2, 3].join
puts ["a", "b", "c"].join("-")
puts [].join(",")
puts [1].join(",")

# Array + and -
puts ([1, 2, 3] + [4, 5]).length
puts ([1, 2, 3] + [4, 5])[3]
diff = [1, 2, 3, 4, 5] - [2, 4]
puts diff.length
puts diff[0]
puts diff[1]
puts diff[2]

# Array#concat — in-place, returns self
base = [1, 2]
result = base.concat([3, 4])
puts base.length    # mutated
puts base[2]
puts result.length  # same object

# take / drop
puts [1, 2, 3, 4, 5].take(3).length
puts [1, 2, 3, 4, 5].take(3)[2]
puts [1, 2, 3, 4, 5].drop(3).length
puts [1, 2, 3, 4, 5].drop(3)[0]
puts [1, 2, 3].take(99).length
puts [1, 2, 3].drop(99).length
puts [1, 2, 3].take(0).length

# to_a is identity on Array
a = [1, 2, 3]
b = a.to_a
puts b.length
puts b[0]

# each_with_index
labels = []
["a", "b", "c"].each_with_index { |v, i| labels << "#{i}:#{v}" }
puts labels[0]
puts labels[1]
puts labels[2]

# sort_by — Ints
nums = [3, 1, 4, 1, 5, 9, 2, 6]
sorted_by_negate = nums.sort_by { |n| -n }
puts sorted_by_negate[0]
puts sorted_by_negate[-1]

# sort_by — strings sorted by length, stable on tie
words = ["pear", "fig", "apple", "kiwi"]
by_len = words.sort_by { |w| w.length }
puts by_len[0]    # "fig" (length 3)
puts by_len[1]    # "pear" or "kiwi" (length 4, first-seen wins)
puts by_len[3]    # "apple" (length 5)

# Chained idioms a real program would write
text = "the quick brown fox the lazy dog the end"
unique_words = text.split.uniq.sort
puts unique_words.length
puts unique_words[0]
puts unique_words[-1]

# Use Array extras inside a class method
class Inventory
  def initialize(items)
    @items = items
  end
  def report
    @items.uniq.sort.join(", ")
  end
end
puts Inventory.new(["apple", "banana", "apple", "cherry", "banana"]).report
