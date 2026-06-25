# `Array.new(n) { block }` must GC-root the accumulating result: the
# block can allocate (here a fresh object per index) and trigger GC
# mid-build, so an unrooted accumulator gets its elements swept ->
# slot recycling -> element aliasing / use-after-free. N is large
# enough to cross the GC threshold several times during the build.
a = Array.new(30_000) { |i| [i, i * 2] }
# Value integrity: every element must still hold its own index — if
# any were swept/aliased mid-build, these would diverge.
puts a.length
puts(a.each_with_index.all? { |pair, i| pair == [i, i * 2] })
puts a.first.inspect
puts a.last.inspect
puts(a.sum { |p| p[0] })
# Distinctness: no two elements may share identity (aliasing symptom).
puts a.map(&:object_id).uniq.length
