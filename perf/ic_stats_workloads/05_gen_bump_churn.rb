# `Op::DefMethod` bumps `method_gen`, invalidating every cached
# entry — re-fill is lazy at each call site. This workload
# alternates a hot dispatch with a `def` redefinition to
# measure how badly gen-bump churn hurts hit rate.
# Expected: hit rate << 1.0 — every redef forces a miss on
# the next dispatch.
N = 1_000
class Mut
  def ping
    1
  end
end
m = Mut.new
i = 0
total = 0
while i < N
  total += m.ping
  # Redefine ping every iteration to bump method_gen.
  class Mut
    def ping
      1
    end
  end
  i += 1
end
puts total
