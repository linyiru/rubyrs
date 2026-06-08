# Enumerator::Lazy — a deferred transform chain over a source that
# responds to each. Nothing runs until forced (first/to_a/force). Built
# as a nested-closure pipeline with throw short-circuit, so infinite
# sources work: take/first never over-iterate.

# headline: infinite source, map + select, first(n)
p (1..Float::INFINITY).lazy.map { |x| x * x }.select { |x| x.even? }.first(5)  # [4,16,36,64,100]
p (1..).lazy.select { |x| x % 3 == 0 }.first(4)            # [3,6,9,12]

# finite force / to_a
p [1, 2, 3, 4, 5].lazy.map { |x| x * 10 }.to_a             # [10,20,30,40,50]
p [1, 2, 3, 4, 5].lazy.select(&:odd?).force               # [1,3,5]
p [1, 2, 3].lazy.reject(&:even?).to_a                     # [1,3]
p [1, 2, 3].lazy.class.to_s                                # "Enumerator::Lazy"

# take / drop (lazy themselves; forced by to_a/first)
p (1..).lazy.map { |x| x * 2 }.take(3).to_a                # [2,4,6]
p (1..10).lazy.drop(7).to_a                                # [8,9,10]
p (1..).lazy.take(5).drop(2).to_a                          # [3,4,5]
p [1, 2, 3].lazy.take(0).to_a                              # []

# take_while / drop_while
p (1..).lazy.take_while { |x| x < 5 }.to_a                 # [1,2,3,4]
p [1, 2, 3, 4, 1, 2].lazy.drop_while { |x| x < 3 }.to_a    # [3,4,1,2]

# filter_map / flat_map
p (1..).lazy.filter_map { |x| x * 2 if x.even? }.first(3)  # [4,8,12]
p [1, 2, 3].lazy.flat_map { |x| [x, -x] }.to_a             # [1,-1,2,-2,3,-3]
p [1, 2, 3].lazy.flat_map { |x| x * 10 }.to_a              # [10,20,30] (non-array passthrough)

# with_index
p %w[a b c].lazy.with_index.to_a                           # [["a",0],["b",1],["c",2]]
p (10..).lazy.with_index.first(3)                          # [[10,0],[11,1],[12,2]]

# longer chains
p (1..).lazy.map { |x| x + 1 }.select(&:even?).map { |x| x * 100 }.first(3)  # [200,400,600]
p (1..Float::INFINITY).lazy.select(&:odd?).map { |x| x * x }.take(4).to_a    # [1,9,25,49]

# Hash / Range / Enumerator sources
p({a: 1, b: 2, c: 3}.lazy.select { |k, v| v.odd? }.to_a)   # [[:a,1],[:c,3]]
p (1..5).lazy.map { |x| x * 2 }.to_a                       # [2,4,6,8,10]
p [1, 2, 3].each.lazy.map { |x| x + 100 }.to_a            # [101,102,103]

# lazy.lazy is self; first with no arg
p [1, 2].lazy.lazy.class.to_s                              # "Enumerator::Lazy"
p (5..).lazy.first                                          # 5
p [].lazy.map { |x| x }.first(3)                          # []
