# Array#values_at (Integer + Range), and the named multi-arg set ops
# intersection / union / difference, plus intersect?.
p [10, 20, 30].values_at(0, 2)
p [1, 2, 3, 4, 5].values_at(0, 2, 4)
p [1, 2, 3].values_at(-1)
p [1, 2, 3, 4, 5].values_at(1..3)
p [1, 2, 3].values_at(5)
p [1, 2, 3, 4, 5].values_at(0, 2..3)
p [1, 2, 3].values_at(0...2)

p [1, 2, 3].intersection([2, 3, 4])
p [1, 2, 3].intersection([2, 3, 4], [3, 4, 5])
p [1, 2, 2, 3].intersection([2, 3])
p [1, 2, 3].union([3, 4])
p [1, 2, 3].union([3, 4], [5])
p [1, 2, 3].difference([2])
p [1, 2, 3, 4].difference([2], [4])
p [1, 2, 3].intersect?([3, 4, 5])
p [1, 2, 3].intersect?([7, 8])
