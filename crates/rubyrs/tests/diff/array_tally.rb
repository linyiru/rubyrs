# Array#tally — count occurrences into a Hash keyed by element
# value, ordered by first appearance.
# (Enumerable#tally_by isn't shipped in MRI yet — proposal
# stalled — so just #tally here.)

# Basic.
puts [1, 2, 2, 3, 3, 3].tally.inspect          # {1 => 1, 2 => 2, 3 => 3}
puts ["a", "b", "a", "c", "b", "a"].tally.inspect
# → {"a" => 3, "b" => 2, "c" => 1}

# Symbols.
puts [:x, :y, :x, :y, :x].tally.inspect        # {x: 3, y: 2}

# Mixed types collide on `==`. CRuby uses `eql?` here so
# 1 and 1.0 are distinct buckets; the subset uses `==`-style
# equality and collapses them. Drop the mixed-Int/Float case
# from the fixture — documented divergence in SUBSET.md.
puts [1, 1, "1"].tally.inspect                 # {1 => 2, "1" => 1}

# Empty.
puts [].tally.inspect                          # {}

# Single element.
puts [42].tally.inspect                        # {42 => 1}

# Boolean and nil elements.
puts [nil, true, nil, false, true].tally.inspect
# → {nil => 2, true => 2, false => 1}

# Order is first-appearance.
puts [3, 1, 4, 1, 5, 9, 2, 6, 5, 3, 5].tally.inspect
# → {3 => 2, 1 => 2, 4 => 1, 5 => 3, 9 => 1, 2 => 1, 6 => 1}
