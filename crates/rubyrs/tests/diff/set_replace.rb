# Set#replace(enum) — clears and re-fills with enum's elements,
# returns self (stdlib_vendor set.rb). zeitwerk's loader uses it.
require "set"
s = Set.new([1, 2, 3])
r = s.replace([4, 5, 6])
p r.equal?(s)
p s.to_a.sort
s.replace(Set.new([9, 9, 8]))
p s.to_a.sort
s.replace([])
p s.empty?
