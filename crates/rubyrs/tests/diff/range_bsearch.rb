# Range#bsearch over an integer range (find-minimum for a boolean
# block, find-any for an Integer block). Driver: parser's tree_rewriter
# does `(from...size).bsearch { |i| ... }` during autocorrection.
p((0...5).bsearch { |i| i >= 3 })       # 3
p((0...5).bsearch { |i| i >= 9 })       # nil
p((2..8).bsearch { |i| i >= 5 })        # 5
p((0...0).bsearch { |i| true })         # nil
p((0..10).bsearch { |i| 4 - i })        # 4 (find-any)
p((0..10).bsearch { |i| 99 - i })       # nil

# the tree_rewriter shape: index into a sorted array via an exclusive range
ch = %w[a b c d e]
idx = (0...ch.size).bsearch { |i| ch[i] >= "c" }
p(idx.nil? ? ch.size : idx)             # 2
p ch.size if (0...ch.size).bsearch { |i| ch[i] >= "z" }.nil?  # 5

p((0...5).bsearch.class)                 # Enumerator (no block)
