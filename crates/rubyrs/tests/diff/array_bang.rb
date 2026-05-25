# Array in-place (`!`) variants — mutate the receiver and return
# self (sort!, reverse!) or self/nil based on whether anything
# changed (uniq!, compact!, flatten!), matching CRuby's contract.

# sort! — always returns self.
arr = [3, 1, 4, 1, 5, 9, 2, 6, 5]
p arr.sort!
p arr

# Already-sorted array — sort! still returns self.
sorted = [1, 2, 3]
p sorted.sort!.equal?(sorted)

# uniq! — nil when nothing deduped.
nodups = [1, 2, 3]
p nodups.uniq!
p nodups

# uniq! with dupes.
dups = [1, 1, 2, 3, 3, 3, 4]
p dups.uniq!
p dups

# compact! — nil when no nils.
clean = [1, 2, 3]
p clean.compact!
p clean

# compact! with nils.
mixed = [1, nil, 2, nil, 3, nil]
p mixed.compact!
p mixed

# flatten! — nil when no nested arrays.
flat = [1, 2, 3]
p flat.flatten!
p flat

# flatten! depth-1 (matching our `flatten` behaviour).
nested = [[1, 2], [3], [4, 5]]
p nested.flatten!
p nested

# reverse! — always returns self.
rev = [1, 2, 3, 4]
p rev.reverse!
p rev

# Chain — sort! then reverse!.
chain = [3, 1, 4, 1, 5, 9, 2, 6]
chain.sort!
chain.reverse!
p chain

# Aliasing: a second variable sees the same mutation.
a = [3, 1, 2]
b = a
a.sort!
p a
p b
p a.equal?(b)

# Mutation inside an iterator-built array.
acc = []
[5, 1, 3, 2, 4].each { |n| acc << n }
acc.sort!
p acc

# Bang variants compose with non-bang chains.
data = [3, 1, 2, 2, 1, 3]
data.uniq!
data.sort!
p data

# Returns nil signals can be used in conditionals.
no_change = [1, 2, 3]
changed = no_change.uniq! ? "changed" : "no-op"
puts changed
