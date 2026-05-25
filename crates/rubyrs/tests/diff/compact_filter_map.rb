# Hash#compact / #compact!, Array#filter_map, Hash#filter_map.
# compact: drop nil-valued entries. compact!: in-place; returns
# self if changed else nil. filter_map: map + select in one pass
# (block result kept iff truthy).

# Hash#compact — non-mutating.
puts({a: 1, b: nil, c: 2, d: nil}.compact.inspect)   # {a: 1, c: 2}
puts({}.compact.inspect)                              # {}
puts({a: 1, b: 2}.compact.inspect)                    # {a: 1, b: 2}  (no nils, returned unchanged in content)

# Hash#compact! — in-place, return value depends on whether
# anything was dropped.
h = {a: 1, b: nil, c: 2}
ret = h.compact!
puts ret.inspect                                      # {a: 1, c: 2}
puts h.inspect                                        # {a: 1, c: 2}

h2 = {a: 1, b: 2}
ret2 = h2.compact!
puts ret2.inspect                                     # nil — no changes
puts h2.inspect                                       # {a: 1, b: 2}

# Array#filter_map — strict truthiness (false is dropped).
puts [1, 2, 3, 4].filter_map { |x| x * 2 if x.even? }.inspect
# → [4, 8]
puts [1, 2, 3].filter_map { |x| nil }.inspect         # []
puts [1, 2, 3].filter_map { |x| false }.inspect       # []
puts [].filter_map { |x| x }.inspect                  # []
puts [nil, 1, nil, 2].filter_map { |x| x }.inspect    # [1, 2]

# Hash#filter_map yields (k, v), collects truthy block results
# into an Array (NOT a Hash, matches CRuby).
puts({a: 1, b: 2, c: 3}.filter_map { |k, v| [k, v * 10] if v > 1 }.inspect)
# → [[:b, 20], [:c, 30]]

# Composition with Hash entries: keep keys whose value passes.
puts({a: 1, b: 2, c: 3, d: 4}.filter_map { |k, v| k if v.even? }.inspect)
# → [:b, :d]
