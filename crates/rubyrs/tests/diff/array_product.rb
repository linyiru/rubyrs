# `Array#product(*others)` — Cartesian product.
#
# Discovery: P3 Sinatra spike — Rack 3 `rack/utils.rb:569`
#   `Hash[((100..199).to_a << 204 << 304).product([true])]`
# is evaluated at module-init time; pre-fix this raised
# `NoMethodError: undefined method 'product' for Array`.

# Shape 1: two arrays — every (a, b) pair.
puts [1, 2].product([3, 4]).inspect

# Shape 2: three arrays — every (a, b, c) triple.
puts [1, 2].product([3], [4, 5]).inspect

# Shape 3: no args — every [e] singleton.
puts [1, 2, 3].product.inspect

# Shape 4: empty self — result is [].
puts [].product([1, 2]).inspect

# Shape 5: empty factor — result is [].
puts [1, 2].product([]).inspect

# Shape 6: empty self + no args — [].
puts [].product.inspect

# Shape 7: iteration order: last factor varies fastest
# (matches CRuby).
puts [:a, :b].product([1, 2], [:x, :y]).inspect

# Shape 8: heterogeneous element types preserved.
puts [1, "s"].product([:k]).inspect

# Shape 9: Rack's actual shape — Hash from product pairs.
status = ((100..102).to_a << 204).product([true])
puts status.inspect
puts Hash[status].inspect

# Shape 10: non-Array arg raises TypeError.
begin
  [1, 2].product(3)
rescue TypeError => e
  puts "type=ok"
end
