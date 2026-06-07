# Set's Enumerable surface (map/select/reject/inject/min/max/sort/
# group_by/...), Set#flatten, and Array#to_set.
require "set"
s = Set.new([3, 1, 2, 2, 4])
p s.map { |x| x * 2 }.sort
p s.select(&:even?).sort
p s.reject(&:even?).sort
p s.min
p s.max
p s.sort
p s.inject(:+)
p s.any? { |x| x > 3 }
p s.group_by(&:even?).transform_values(&:sort)
p Set.new([1, Set.new([2, Set.new([3])]), 4]).flatten.sort
p [1, 2, 2, 3].to_set.size
