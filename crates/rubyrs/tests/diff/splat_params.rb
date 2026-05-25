# Method-param splat (`def f(a, *rest)`). Differential-tested against
# CRuby so any divergence in arg distribution surfaces immediately.

# Bare rest only — every arg lands in the Array.
def all_in(*xs)
  xs
end
puts all_in.inspect
puts all_in(1).inspect
puts all_in(1, 2, 3).inspect

# Required prefix + rest.
def head_tail(a, *rest)
  [a, rest]
end
puts head_tail(1).inspect
puts head_tail(1, 2).inspect
puts head_tail(1, 2, 3, 4).inspect

# Required + optional + rest. Optional fills before rest collects.
def mixed(a, b = 99, *rest)
  [a, b, rest]
end
puts mixed(1).inspect
puts mixed(1, 2).inspect
puts mixed(1, 2, 3).inspect
puts mixed(1, 2, 3, 4, 5).inspect

# Two required + rest — rest stays empty when exactly two args.
def pair_then_rest(a, b, *rest)
  [a, b, rest.length, rest]
end
puts pair_then_rest(10, 20).inspect
puts pair_then_rest(10, 20, 30, 40).inspect

# Rest used in the body (length / first / each via inject-shape).
def sum_all(*xs)
  total = 0
  xs.each { |x| total = total + x }
  total
end
puts sum_all
puts sum_all(7)
puts sum_all(1, 2, 3, 4, 5)
