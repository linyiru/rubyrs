# Single self-ivar read (slot 0, scan = 1 iteration) in a tight native loop — isolates the
# jit_self_ivars slab lookup + a minimal scan, vs treesum's 3 reads (1+2+3 scan iters).
class Box
  def initialize(x); @x = x; end
  def hammer(n); acc = 0; i = 0; while i < n; acc += @x; i += 1; end; acc; end
end
b = Box.new(7)
b.hammer(1000)
N = 3000
t = Time.now; acc = 0
N.times { acc += b.hammer(100000) }
dt = Time.now - t
puts "getter @x in native loop: per_iter=%.4fms (acc=%d)" % [dt*1000.0/N, acc]
