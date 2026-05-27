# 4-shape polymorphic dispatch — alternates among 4 classes,
# which is exactly `IC_WAYS = 4`. Expected: hit rate ~ 0.999
# after the first cycle (each way fills, all subsequent hits).
N = 10_000
class A; def tag; 'a'; end; end
class B; def tag; 'b'; end; end
class C; def tag; 'c'; end; end
class D; def tag; 'd'; end; end
shapes = [A.new, B.new, C.new, D.new]
i = 0
seen = 0
while i < N
  seen += 1 if shapes[i % 4].tag.length > 0
  i += 1
end
puts seen
