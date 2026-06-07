# Set proper subset/superset operators, add?/delete?, each_with_index
# return value, and order-independent #hash.
require "set"
a = Set.new([1, 2])
b = Set.new([1, 2, 3])
p(a < b)                  # true (proper subset)
p(b < b)                  # false (not proper)
p(b > a)                  # true (proper superset)
p(a > b)                  # false
p a.proper_subset?(b)     # true
p b.proper_superset?(a)   # true

s = Set.new([1])
p s.add?(2).class         # Set (added)
p s.add?(2)               # nil (already present)
p s.include?(2)           # true
p s.delete?(2).class      # Set (deleted)
p s.delete?(99)           # nil (absent)
p s.include?(2)           # false

orig = Set.new([10, 20])
r = orig.each_with_index { |_e, _i| }
p r.equal?(orig)          # true (returns the receiver)

# #hash is order-independent.
p(Set.new([1, 2, 3]).hash == Set.new([3, 2, 1]).hash)   # true
