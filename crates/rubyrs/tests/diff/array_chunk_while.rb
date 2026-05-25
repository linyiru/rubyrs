# Enumerable#chunk_while — partition into consecutive runs where
# the 2-arg block `{|a,b| ...}` is truthy for adjacent pairs.
# CRuby returns an Enumerator; the subset returns the Array
# directly (no Enumerator type — see SUBSET.md). Both shapes
# work with `.to_a` for portability, which is what the fixture
# uses.

# Equal-run grouping.
puts [1, 1, 2, 2, 3].chunk_while { |a, b| a == b }.to_a.inspect
# → [[1,1], [2,2], [3]]

# Consecutive-integer runs.
puts [1, 2, 4, 9, 10, 11, 12, 15, 16, 19, 20, 21].chunk_while { |a, b| b - a == 1 }.to_a.inspect
# → [[1,2], [4], [9,10,11,12], [15,16], [19,20,21]]

# Edge: empty / single.
puts [].chunk_while { |a, b| true }.to_a.inspect      # []
puts [42].chunk_while { |a, b| true }.to_a.inspect    # [[42]]

# Strictly-ascending runs.
puts [3, 1, 4, 1, 5, 9, 2, 6, 5, 3].chunk_while { |a, b| a < b }.to_a.inspect
# → [[3],[1,4],[1,5,9],[2,6],[5],[3]]

# All-true predicate → one big chunk.
puts [1, 2, 3, 4, 5].chunk_while { |a, b| true }.to_a.inspect
# → [[1,2,3,4,5]]

# All-false predicate → each element its own chunk.
puts [1, 2, 3, 4].chunk_while { |a, b| false }.to_a.inspect
# → [[1],[2],[3],[4]]
