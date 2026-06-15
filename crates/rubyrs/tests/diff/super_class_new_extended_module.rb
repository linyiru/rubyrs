# `super(*args, &block)` from a `new` defined in an EXTENDED module must
# reach the builtin `Class#new` (allocate + initialize, forwarding the
# block). Surfaced by concurrent-ruby's SafeInitialization extended onto
# Concurrent::Delay.
module SafeInit
  def new(*args, &block)
    super(*args, &block)
  ensure
    # (full_memory_barrier — no-op here)
  end
end

class Delay
  extend SafeInit
  def initialize(x, y)
    @x = x
    @y = y
  end
  def sum; @x + @y; end
end
d = Delay.new(40, 2)
p d.sum

# Block forwarded through super(*a, &b) to Class#new → initialize.
class WithBlock
  extend SafeInit
  def initialize(n)
    @v = yield(n)
  end
  def v; @v; end
end
w = WithBlock.new(10) { |n| n * 3 }
p w.v

# No-arg / no-block super still allocates + initializes.
class Plain
  extend SafeInit
  def initialize; @z = :set; end
  def z; @z; end
end
p Plain.new.z
