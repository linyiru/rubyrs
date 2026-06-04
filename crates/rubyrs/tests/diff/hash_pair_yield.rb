# Hash iteration auto-array-wrap for arity-1 blocks. CRuby's
# rule for most Hash iter methods (each, map, collect, find,
# sort_by, etc.): the block receives `[k, v]` as a single pair
# Array. Two-arg blocks auto-destructure via the F4 prologue;
# single-arg blocks receive the pair Array directly.
#
# `Hash#select` / `#reject` / `#filter` deliberately override
# this — they yield `(k, v)` as two args, so arity-1 binds to
# just `k`. The discrimination is encoded in `iter_hash_filter`.

h = {a: 1, b: 2, c: 3}

# `collect` / `map` — single-arg block yields pair Array.
p h.collect { |m| m }
p h.collect { |m| m.class }
p h.collect { |k, v| [k.to_s, v * 10] }

# `each` — same shape, but returns the hash.
parts = []
h.each { |m| parts << m.inspect }
puts parts.join(" | ")

# `each_pair` — alias.
parts = []
h.each_pair { |m| parts << m.inspect }
puts parts.join(" | ")

# Destructure block `|(k, v)|` also lands correctly.
out = h.collect { |(k, v)| "#{k}=#{v}" }
p out

# `filter_map` — yields pair Array, keeps truthy.
p h.filter_map { |m| m[1] >= 2 ? m[0].to_s : nil }

# `sort_by` — yields pair, sort key from block return.
p h.sort_by { |m| -m[1] }

# `group_by` — yields pair, group key from block return.
p h.group_by { |m| m[1] >= 2 ? :high : :low }

# `min_by` / `max_by` — yields pair, block returns the sort key.
p h.min_by { |m| m[1] }
p h.max_by { |m| m[1] }

# Hash#select / #reject / #filter — DIFFERENT shape: arity-1
# binds to k only (no auto-wrap). These methods override
# Enumerable's pair-yield shape in CRuby.
p h.select { |k| k != :b }
p h.reject { |k| k == :a }
p h.filter { |k, v| v > 1 }

# `find` / `detect` (Enumerable) — pair-yield like map.
p h.find { |m| m[1] == 2 }

# `any?` / `all?` / `none?` (Enumerable) — pair-yield.
p h.any? { |m| m[1] >= 2 }
p h.all? { |m| m[1] >= 1 }
p h.none? { |m| m[1] >= 4 }

# Two-arg block on map still works (auto-destructure).
p h.map { |k, v| [k, v + 100] }
