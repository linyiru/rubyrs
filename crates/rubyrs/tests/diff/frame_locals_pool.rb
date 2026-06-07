# Frame-locals pooling must never recycle a cell a closure still
# captures: a lambda over a method's locals keeps its own values even as
# a flood of unrelated calls recycle pooled cells (the strong_count
# guard). Also exercises deep recursion that pushes/pops/recycles many
# frames.
def gen(i)
  x = i * 10
  -> { x }
end
procs = []
5.times { |i| procs << gen(i) }

def noop(a)
  b = a + 1
  b
end
2000.times { noop(7) }            # recycle many pooled cells in between
p procs.map(&:call)               # [0, 10, 20, 30, 40] — uncorrupted

def make_counter
  count = 0
  -> { count += 1 }
end
c = make_counter
1000.times { noop(7) }
p [c.call, c.call, c.call]        # [1, 2, 3]

# Mutually nested closures over distinct frames.
def outer(n)
  total = 0
  adder = ->(k) { total += k }
  n.times { |j| adder.call(j) }
  total
end
p [outer(5), outer(10)]           # [10, 45]

def fib(n)
  n < 2 ? n : fib(n - 1) + fib(n - 2)
end
p fib(20)                          # 6765
