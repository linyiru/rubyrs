# `Kernel#p(a, b, ...)` with three-or-more heap-bearing args
# returns a fresh Array of those args. Under STRESS_GC=1 the
# return path is delicate: `args` is a `&[Value]` borrow of a
# Vec drained out of `self.stack`, so the elements are not in
# any GC root set. If `maybe_gc` runs between the match-arm
# binding (`many => ...`) and `heap.alloc`, the slots get
# reaped and the new Array references recycled storage — the
# same shape as the seven sites from issue #90.
#
# This fixture stresses the path so STRESS_GC=1 flushes any
# regression in the `p` / `pp` return path. We stick to Value
# kinds (String, Array, Hash) whose inspect output is identical
# between rubyrs and CRuby — user-defined `inspect` on custom
# classes is a separate subset boundary, out of scope here.

200.times do
  a = p("alpha", "beta", "gamma")
  raise "p string multi-arg broken" unless a.length == 3 && a[0] == "alpha"
end

200.times do
  a = p([1, 2], "mid", [3, 4])
  raise "p array multi-arg broken" unless a.length == 3 && a[1] == "mid"
end

200.times do
  a = p({k: 1}, {k: 2}, {k: 3})
  raise "p hash multi-arg broken" unless a.length == 3
end
