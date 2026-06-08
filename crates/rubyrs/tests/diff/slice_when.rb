# Enumerable#slice_when — split between consecutive elements where the
# block is truthy (the inverse of chunk_while, which keeps them together
# while truthy). Same driver, opposite test, across Array/Hash/Range.
# (Returns the Array of runs; CRuby returns a lazy Enumerator — `.to_a`
# is portable. respond_to?(:slice_when) is now true on all three.)

# Array
p [1, 2, 4, 5, 7].slice_when { |a, b| b - a > 1 }.to_a   # [[1,2],[4,5],[7]]
p [1, 1, 2, 3, 3].slice_when { |a, b| a != b }.to_a      # [[1,1],[2],[3,3]]
p [1, 2, 3].slice_when { |a, b| false }.to_a             # [[1,2,3]]
p [1, 2, 3].slice_when { |a, b| true }.to_a              # [[1],[2],[3]]
p [].slice_when { |a, b| true }.to_a                     # []
p [42].slice_when { |a, b| true }.to_a                   # [[42]]
# inverse-of-chunk_while identity
p [1, 2, 4, 5].slice_when { |a, b| b - a > 1 }.to_a == [1, 2, 4, 5].chunk_while { |a, b| b - a == 1 }.to_a  # true

# Range
p (1..6).slice_when { |a, b| b.even? }.to_a              # [[1],[2,3],[4,5],[6]]
p (1..5).slice_when { |a, b| false }.to_a                # [[1,2,3,4,5]]

# Hash (pairs)
h = {a: 1, b: 1, c: 2, d: 2}
p h.slice_when { |x, y| x[1] != y[1] }.to_a              # [[[:a,1],[:b,1]],[[:c,2],[:d,2]]]

# respond_to? lockstep
p [1].respond_to?(:slice_when)                           # true
p({}.respond_to?(:slice_when))                           # true
p (1..2).respond_to?(:slice_when)                        # true


# arity guard (with block)
begin; [1].slice_when(1) { |a, b| true }; rescue => e; puts "#{e.class}: #{e.message}"; end
