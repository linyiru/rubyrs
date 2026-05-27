# 4-shape polymorphic dispatch — alternates among 4 classes,
# well under `IC_WAYS = 5`. Expected: hit rate ~ 0.999 after the
# first cycle (each way fills, all subsequent hits). Filename
# describes the workload shape (4 user classes), not the IC
# width.
#
# The accumulator USES the dispatch result (rather than a
# tautological `cond > 0` guard) so a future DCE pass that
# constant-folds non-empty-literal length comparisons can't
# silently strip the dispatch and leave the workload measuring
# nothing.
N = 10_000
class A; def tag; 'a'; end; end
class B; def tag; 'b'; end; end
class C; def tag; 'c'; end; end
class D; def tag; 'd'; end; end
shapes = [A.new, B.new, C.new, D.new]
i = 0
total = 0
while i < N
  total += shapes[i % 4].tag.length
  i += 1
end
puts total
