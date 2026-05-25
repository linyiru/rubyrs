# min_by / max_by — block returns the comparison key
puts [1, -3, 2, -5, 4].min_by { |x| x.abs }      # 1
puts [1, -3, 2, -5, 4].max_by { |x| x.abs }      # -5
puts ["aa", "b", "ccc"].min_by { |s| s.length } # "b"
puts ["aa", "b", "ccc"].max_by { |s| s.length } # "ccc"

# Tie-breaking: first element with the min/max key wins
puts ["alpha", "beta", "gamma", "delta"].min_by { |s| s.length }  # "beta"
puts ["a", "b", "cc", "dd"].max_by { |s| s.length }               # "cc"

# Empty array — nil
puts [].min_by { |x| x }.nil?
puts [].max_by { |x| x }.nil?

# Single-element
puts [42].min_by { |x| x }
puts [42].max_by { |x| x }

# Negative-key projection
puts [10, 20, 30].min_by { |x| -x }   # 30 (smallest -x)
puts [10, 20, 30].max_by { |x| -x }   # 10

# group_by — Hash {key => [elements...]}
h = [1, 2, 3, 4, 5, 6].group_by { |n| n % 2 }
puts h.size
puts h[1].length
puts h[0].length
puts h[1][0]
puts h[1][1]
puts h[1][2]
puts h[0][0]
puts h[0][1]
puts h[0][2]

# group_by — Symbol keys
words = ["apple", "ant", "banana", "blueberry", "cherry"]
by_initial = words.group_by { |w| w.chars[0] }
puts by_initial["a"].length
puts by_initial["b"].length
puts by_initial["c"].length
puts by_initial["a"][0]
puts by_initial["a"][1]
puts by_initial["b"][1]

# Empty input -> empty Hash
puts({}.size)
g = [].group_by { |x| x }
puts g.size

# Chained min_by inside a method with default args
def shortest(words, fallback = "(none)")
  return fallback if words.empty?
  words.min_by { |w| w.length }
end
puts shortest(["one", "two", "three", "four"])
puts shortest([])

# respond_to?
puts [].respond_to?(:min_by)
puts [].respond_to?(:max_by)
puts [].respond_to?(:group_by)
