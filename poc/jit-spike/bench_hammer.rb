# Isolates the GENERIC obj-call (ADR 0035 Phase 3b) in a tight NATIVE while-loop:
# hammer(c, n) is a 2-param method (Object c, Int n); `c.fee(i)` is the generic obj-call.
class Calc
  def initialize(b); @b = b; end
  def fee(r); @b * 3 + r - 1; end
end
def hammer(c, n)
  acc = 0
  i = 0
  while i < n
    acc += c.fee(i)
    i += 1
  end
  acc
end
c = Calc.new(7)
hammer(c, 1000)  # warmup
N = 3000
t = Time.now
acc = 0
N.times { acc += hammer(c, 100000) }
dt = Time.now - t
puts "hammer c.fee in native loop: N=%d total=%.3fs per_iter=%.4fms (acc=%d)" % [N, dt, dt*1000.0/N, acc]
