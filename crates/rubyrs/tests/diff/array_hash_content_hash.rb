# Array#hash and Hash#hash content-based — the universal
# Object#hash arm from PR #272 fell through to the
# identity-based heap branch for Array and Hash, so two
# distinct-but-equal allocations produced different hashes.
# CRuby hashes by content; Set/Hash with Array or Hash keys
# would otherwise silently miss.
#
# PR #272 already shipped a `range_hash.rb` fixture using the
# `object_hash` helper for Range. This commit extends that
# helper with two more arms:
#
#   Array (tag 9, order-sensitive)
#   Hash  (tag 10, order-insensitive)
#
# plus a small cycle-detection guard so `a = []; a << a;
# a.hash` doesn't infinite-recurse.

# Array — same content → same hash
puts [1, 2, 3].hash == [1, 2, 3].hash
puts [].hash == [].hash
puts ["a", "b"].hash == ["a", "b"].hash

# Array — order-sensitive
puts [1, 2, 3].hash != [3, 2, 1].hash
puts [1, 2].hash != [1, 2, 3].hash

# Array — nested
puts [[1, 2], [3, 4]].hash == [[1, 2], [3, 4]].hash
puts [[1, 2], [3, 4]].hash != [[3, 4], [1, 2]].hash

# Hash — same content → same hash
puts({a: 1, b: 2}.hash == {a: 1, b: 2}.hash)
puts({}.hash == {}.hash)

# Hash — order-INsensitive (CRuby parity: {a:1,b:2} ==
# {b:2,a:1} and hashes must agree)
puts({a: 1, b: 2}.hash == {b: 2, a: 1}.hash)

# Hash — content-distinct hashes distinct
puts({a: 1}.hash != {a: 2}.hash)
puts({a: 1}.hash != {b: 1}.hash)

# Hash — pair-internal swap must perturb (regression guard for
# the cycle-2 XOR-collision finding). A bare `kh ^ vh` per pair
# would collide `{1=>2,2=>1}` with `{1=>1,2=>2}` because both
# reduce to `acc = 0` despite being `!=`. The combinator now
# mixes key and value non-symmetrically (`kh*31 + vh`).
puts({1 => 2, 2 => 1}.hash != {1 => 1, 2 => 2}.hash)
puts({1 => 2, 2 => 1}.hash == {2 => 1, 1 => 2}.hash)

# Mixed nesting
puts({a: [1, 2]}.hash == {a: [1, 2]}.hash)
puts([{x: 1}, {y: 2}].hash == [{x: 1}, {y: 2}].hash)

# As Hash key (vm/hash.rs does linear-scan + ruby_eql, so this
# would work even with identity hash — but the contract that
# content-equal keys behave as one key is what users expect).
h = { [1, 2] => :a, {x: 1} => :b }
puts h[[1, 2]]
puts h[{x: 1}]

# Cycle: `a << a` must hash without stack overflow
a = []
a << a
puts a.hash == a.hash
puts a.hash.is_a?(Integer)

# Hash containing itself
hh = {}
hh[:self] = hh
puts hh.hash == hh.hash
puts hh.hash.is_a?(Integer)
