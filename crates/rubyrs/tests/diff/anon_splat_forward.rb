# Ruby 3.x anonymous splat forwarding: `def m(*)` binds an unnamed rest,
# and `yield(*)` / `other(*)` forward it. Surfaced by bridgetown-core's
# erb_templates.rb (`def capture(*); yield(*); end`).
def capture(*)
  yield(*)
end
puts capture(1, 2, 3) { |a, b, c| "#{a}-#{b}-#{c}" }

# anon splat forwarded to a regular call
def combine(*)
  sum(*)
end
def sum(*nums)
  nums.sum
end
puts combine(4, 5, 6)

# leading positional + anon splat at a yield
def mixed(*)
  yield(:head, *)
end
puts mixed(7, 8) { |a, b, c| [a, b, c].inspect }

# empty forward
def none(*)
  sum(*)
end
puts none
