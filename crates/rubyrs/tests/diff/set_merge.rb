# Set#merge(*enums) folds each enumerable's elements into self
# in place, returning self. Discovery: P3 Jekyll spike — Liquid's
# strainer.rb merges a filter module's methods into the global set.
require "set"
s = Set.new([1, 2])
r = s.merge([2, 3, 4])
p r.equal?(s)              # returns self
p s.include?(3)
p s.size
s.merge([5], [6, 7])      # multiple enums
p s.size
s.merge(Set.new([8, 1]))  # merging another Set; dups ignored
p s.to_a.sort
empty = Set.new
empty.merge([])
p empty.size
