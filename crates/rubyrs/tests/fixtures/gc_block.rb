nums = []
i = 0
while i < 2000
  nums << i
  i = i + 1
end

# Each block iteration allocates a fresh inner Array; without a GC root for
# `map`'s accumulating result, the early ones get swept after the heap
# threshold is crossed. Reading result[0] etc. should work.
result = nums.map { |x| [x, x * 2] }
puts result.length
puts result[0][0]
puts result[0][1]
puts result[500][0]
puts result[1999][1]

# Hash#each with allocations inside the block — exercises Hash pin path.
counts = {a: 0, b: 0}
[1, 2, 3, 4, 5].each { |x| counts[:a] = counts[:a] + 1 }
puts counts[:a]

# each with block that returns a fresh array — exercises Array pin path
# under stress mode.
seen = []
[1, 2, 3].each { |x| seen << [x, x] }
puts seen.length
puts seen[1][0]
