# Set#collect! / #map! — in-place map (replace each element with the
# block's result), return self; no-block → Enumerator. rouge's isbl lexer
# uses `Set#collect!`.
require "set"
s = Set.new([1, 2, 3])
ret = s.collect! { |x| x * 10 }
p s.to_a.sort                          # [10, 20, 30]
p ret.equal?(s)                        # true (returns self)
s2 = Set.new([1, 2, 3, 4])
s2.map! { |x| x % 2 }
p s2.to_a.sort                         # [0, 1] (dedup after map)
s3 = Set.new(%w[a bb ccc])
s3.collect! { |x| x.length }
p s3.to_a.sort                         # [1, 2, 3]
p Set.new([1, 2]).collect!.class.to_s  # "Enumerator"
empty = Set.new
empty.map! { |x| x }
p empty.to_a                           # []
