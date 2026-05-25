# method_missing dispatch microbench. Every call goes through the
# class chain to method_missing, which echoes the symbol name back.
# 2,000,000 iterations — sized to amortise CRuby's ~50 ms boot so
# the timing reflects steady-state per-call dispatch cost rather
# than cold start. See README.md for numbers.

class Ghost
  def method_missing(name)
    name
  end
end

g = Ghost.new
n = 2_000_000
sink = nil
i = 0
while i < n
  sink = g.this_method_does_not_exist
  i = i + 1
end
puts sink
