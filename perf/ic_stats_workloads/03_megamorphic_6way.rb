# 6-shape megamorphic dispatch — cycles among 6 classes, one
# past the current `IC_WAYS = 5`. Expected: hit rate ≈ 0.5 —
# the round-robin eviction means every dispatch on a
# recently-evicted shape misses and re-fills, thrashing all
# five ways. This workload is the cliff guard: the moment a
# real corpus shows a hot site with 6+ shapes, you'll see it
# here first.
#
# Was 5 shapes against `IC_WAYS = 4` in PR #175 (hit rate
# 0.4998); widened to 6 alongside the `IC_WAYS = 4 → 5` bump
# so the workload still measures the design's stress point
# rather than the comfortable case.
#
# Accumulator pattern matches workload 02 — load-bearing
# dispatch result so a future DCE pass can't silently strip
# the hot site.
N = 10_000
class A; def tag; 'a'; end; end
class B; def tag; 'b'; end; end
class C; def tag; 'c'; end; end
class D; def tag; 'd'; end; end
class E; def tag; 'e'; end; end
class F; def tag; 'f'; end; end
shapes = [A.new, B.new, C.new, D.new, E.new, F.new]
i = 0
total = 0
while i < N
  total += shapes[i % 6].tag.length
  i += 1
end
puts total
