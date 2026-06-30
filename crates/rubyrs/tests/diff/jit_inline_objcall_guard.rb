# ADR 0035 Phase 3 — the inline class-guard fast path for a GENERIC obj-call (`o.f(r)` where
# `o` is a materialized Object param). Exercises: the fast path (monomorphic, cache hit), the
# slow-path fallback (POLYMORPHIC same call site → cache miss → recompile + deopt), and the
# native-loop hammer. Parity must hold interpreter == JIT == CRuby.

class A
  def initialize(x); @x = x; end
  def f(r); @x + r; end
end
class B
  def initialize(x); @x = x; end
  def f(r); @x * r; end
end

def call_f(o, r)   # 2-param method: Object o, Int r → o.f(r) is the generic obj-call
  o.f(r)
end

a = A.new(10)
b = B.new(3)

# Warm the call site monomorphic (A) → fast path.
p call_f(a, 5)   # 15
p call_f(a, 7)   # 17
# Now switch class at the SAME call site (PIC miss → slow path recompiles for B).
p call_f(b, 4)   # 12
p call_f(b, 2)   # 6
# Back to A (miss again → slow path).
p call_f(a, 1)   # 11

# Heavy monomorphic loop (fast path dominates) — A then B.
sa = 0
100_000.times { |i| sa += call_f(a, i) }
p sa
sb = 0
100_000.times { |i| sb += call_f(b, i) }
p sb

# Alternating polymorphic loop (cache thrashes hit/miss every iteration).
sp = 0
1000.times { |i| sp += call_f(i.even? ? a : b, i) }
p sp
