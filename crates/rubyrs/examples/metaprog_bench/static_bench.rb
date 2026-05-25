# Baseline for `dm_bench.rb`: same body but installed via `def` so the
# method has a fixed proto and no captured closure. Comparing the two
# isolates the closure-method dispatch overhead in rubyrs.

class Bumper
  def initialize
    @state = 0
  end
  def bump
    @state = @state + 1
    @state
  end
end

b = Bumper.new
n = 2_000_000
sink = nil
i = 0
while i < n
  sink = b.bump
  i = i + 1
end
puts sink
