# Range#hash — added when the universal Object#hash arm from
# PR #272 was found (via /code-review post-merge audit) to fall
# through to the identity-based `_ => object_id_for(...)` branch
# for Range, making distinct allocations of `(1..5)` produce
# different hashes. CRuby hashes Range by content (begin, end,
# exclusive flag), so semantically-equal ranges must hash equal.

# Same content → same hash
puts (1..5).hash == (1..5).hash
puts (1...5).hash == (1...5).hash
puts ("a".."z").hash == ("a".."z").hash

# Different content → different hash (best-effort, may collide
# in principle but DefaultHasher across 64 bits makes that
# vanishingly unlikely for these tiny inputs).
puts (1..5).hash != (2..5).hash
puts (1..5).hash != (1..6).hash

# Exclusive flag participates in the hash — `..` vs `...`
# distinguish.
puts (1..5).hash != (1...5).hash

# Range as Hash key — even though vm/hash.rs uses linear-scan
# with `ruby_eql` and doesn't strictly need #hash for lookup,
# the content-hash contract is what users expect.
h = { (1..5) => :a, ("a".."z") => :b }
puts h[1..5]
puts h["a".."z"]

# Endless / beginless ranges hash by content too.
puts (1..).hash == (1..).hash
puts (..5).hash == (..5).hash
puts (1..).hash != (..5).hash
