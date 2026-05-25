# Array#combination(n) and Array#permutation([n]) — non-block,
# materialised Array of Arrays (no Enumerator in the subset).
# Edge cases match CRuby: n=0 → [[]]; n > length → [].

# combination
puts [1, 2, 3].combination(2).to_a.inspect
# → [[1,2], [1,3], [2,3]]
puts [1, 2, 3, 4].combination(3).to_a.inspect
# → [[1,2,3], [1,2,4], [1,3,4], [2,3,4]]
puts [1, 2, 3].combination(1).to_a.inspect    # [[1], [2], [3]]
puts [1, 2, 3].combination(0).to_a.inspect    # [[]]
puts [1, 2, 3].combination(5).to_a.inspect    # []
puts [].combination(0).to_a.inspect           # [[]]
puts [].combination(1).to_a.inspect           # []

# permutation
puts [1, 2, 3].permutation(2).to_a.inspect
# → [[1,2], [1,3], [2,1], [2,3], [3,1], [3,2]]
puts [1, 2, 3].permutation.to_a.inspect       # full = permutation(3)
# → [[1,2,3], [1,3,2], [2,1,3], [2,3,1], [3,1,2], [3,2,1]]
puts [1, 2].permutation(0).to_a.inspect       # [[]]
puts [1, 2, 3].permutation(1).to_a.inspect    # [[1], [2], [3]]
puts [1, 2, 3].permutation(5).to_a.inspect    # []
puts [].permutation.to_a.inspect              # [[]]

# Realistic: choose 2 of 4.
people = ["Alice", "Bob", "Cara", "Dan"]
puts people.combination(2).to_a.length        # 6
