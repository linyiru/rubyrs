# Divergence ratchet: Hash key equality uses `==`, not `eql?`.
#
# CRuby's Hash distinguishes `1.0` from `1` (1.0.eql?(1) is false),
# so `{ a: 1.0, b: 1 }.invert` keeps two entries: `{ 1.0 => :a, 1 => :b }`.
# rubyrs's Hash uses numeric `==` for key comparison, so
# `{ a: 1.0, b: 1 }.invert` collapses to one entry, the later write
# wins: `{ 1.0 => :b }`.
#
# When fixed (Hash key lookup in vm/hash.rs grows Float/Int eql?
# distinction), regen this fixture via UPDATE_EXPECTED=1 AND
# un-skip the `# skipped (divergent): "compares new keys with eql?
# semantics"` block in `spec/ruby/hash_invert_spec.rb`.

# Build the {Symbol → Number} hash and invert. The result's size and
# values both diverge between CRuby and rubyrs.
h = { a: 1.0, b: 1 }
i = h.invert
puts "size: #{i.size}"

# Querying the inverted hash by either numerically-equal key.
puts "i[1.0]: #{i[1.0].inspect}"
puts "i[1]:   #{i[1].inspect}"

# Direct same-shape hash literal that lays bare the collision.
literal = { 1.0 => :a, 1 => :b }
puts "literal size: #{literal.size}"
puts "literal[1.0]: #{literal[1.0].inspect}"
puts "literal[1]:   #{literal[1].inspect}"
