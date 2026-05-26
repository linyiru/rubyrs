# Set vendored as Tier 3 pure-Ruby stdlib (subset).
# Hash-backed; covers the deterministic core that CRuby's
# stdlib/set.rb exposes via its top-level Set constant. Not
# modelled: SortedSet, RestrictedSet, Comparable-from-Set,
# Marshal hooks — those reach for stdlib niceties we don't
# carry. Scripts that touch them get NoMethodError, which is
# the right "feature absent" surface.
#
# Fixture runs under `--features stdlib` only (registered as
# `#[cfg(feature = "stdlib")]` in tests/diff_cruby.rs).

require 'set'

# Class identity.
puts Set.class.name              # "Class"
puts Set.new.class.name          # "Set"

# Construction.
empty = Set.new
puts empty.empty?                # true
puts empty.size                  # 0

from_arr = Set.new([1, 2, 3, 2, 1])
puts from_arr.size               # 3 (dedup)
puts from_arr.include?(1)        # true
puts from_arr.include?(99)       # false

# add / << — return self.
s = Set.new
s.add(10)
s << 20 << 30
puts s.size                      # 3
puts s.to_a.inspect              # insertion order

# member? / include? aliases.
puts s.member?(20)               # true

# size / length aliases.
puts s.length                    # 3

# delete / clear.
s.delete(20)
puts s.include?(20)              # false
puts s.size                      # 2
s.clear
puts s.empty?                    # true

# Iteration — block + collected.
collected = []
Set.new([:a, :b, :c]).each { |x| collected << x }
puts collected.inspect           # [:a, :b, :c]

# Equality + eql?.
a = Set.new([1, 2, 3])
b = Set.new([3, 2, 1])           # same content, different insertion order
c = Set.new([1, 2])
puts a == b                      # true (content-based)
puts a.eql?(b)                   # true
puts a == c                      # false
puts a == [1, 2, 3]              # false (Set != Array)

# Set algebra.
x = Set.new([1, 2, 3])
y = Set.new([3, 4, 5])
puts (x | y).to_a.inspect        # union: [1, 2, 3, 4, 5]
puts (x + y).to_a.inspect        # union alias
puts x.union(y).to_a.inspect     # named alias
puts (x - y).to_a.inspect        # difference: [1, 2]
puts x.difference(y).to_a.inspect
puts (x & y).to_a.inspect        # intersection: [3]
puts x.intersection(y).to_a.inspect

# subset / superset.
small = Set.new([1, 2])
big   = Set.new([1, 2, 3, 4])
puts small.subset?(big)          # true
puts big.subset?(small)          # false
puts small <= big                # true
puts big.superset?(small)        # true
puts big >= small                # true

# Mixed-element types (Set keys by .hash + .eql?).
mix = Set.new
mix.add("hello")
mix.add(:hello)
mix.add(42)
puts mix.size                    # 3 — String, Symbol, Int all distinct

# `Set#hash` not modelled (Tier 1 Integer / Symbol don't expose
# .hash); equality still works through `Set#==`'s include-based
# walk. See vendor source for the rationale comment.
