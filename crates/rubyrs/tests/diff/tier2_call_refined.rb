# Tier-2 wave-2: a refined method name must NEVER be served by the IC-fast
# t2_call path (ADR 0037) — the helper's refinement gate falls back to the
# full dispatch, which owns refinement resolution. The Driver bodies below
# are warmed far past the tier-2 compile threshold, so under
# RUBYRS_JIT_TIER2=1 every hot call to the REFINED names (`tag`, `calc`)
# runs through a compiled caller whose call op must decline to the cascade
# on each execution, while the UNREFINED name (`plain`) on the same
# receiver keeps the IC-fast serve — the mixed loop checks both stay exact.

class Node
  def tag = "base"
  def calc(x) = x + 1
  def plain = 7
end

module Sharper
  refine Node do
    def tag = "refined"
    def calc(x) = x * 100
  end
end

using Sharper

class Driver
  def drive(n) = n.tag
  def drive2(n, x) = n.calc(x)
  def drive3(n) = n.plain
end

d = Driver.new
n = Node.new
puts d.drive(n)
puts d.drive2(n, 2)
puts d.drive3(n)
acc = 0
5000.times { acc += d.drive(n).size + d.drive2(n, 1) + d.drive3(n) }
puts acc
puts n.tag
puts n.calc(3)
