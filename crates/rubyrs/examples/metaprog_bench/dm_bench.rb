# define_method dispatch microbench. The accessor is installed via
# `define_method` at class-body time and shares closure-state with
# the surrounding scope (the counter ticks across calls).

class Bumper
  state = 0
  define_method(:bump) { state = state + 1; state }
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
