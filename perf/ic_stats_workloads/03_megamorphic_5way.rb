# 5-shape megamorphic dispatch — cycles among 5 classes,
# exceeding `IC_WAYS = 4`. Expected: hit rate < 0.6 — the
# round-robin eviction means every dispatch on a recently-
# evicted shape misses and re-fills, thrashing all 4 ways.
# This is the workload that would benefit from widening
# IC_WAYS to 5 (or LRU eviction).
N = 10_000
class A; def tag; 'a'; end; end
class B; def tag; 'b'; end; end
class C; def tag; 'c'; end; end
class D; def tag; 'd'; end; end
class E; def tag; 'e'; end; end
shapes = [A.new, B.new, C.new, D.new, E.new]
i = 0
seen = 0
while i < N
  seen += 1 if shapes[i % 5].tag.length > 0
  i += 1
end
puts seen
