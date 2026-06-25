# Hash#shift removes the first [k,v] pair and allocates the result
# Array; k and v were just removed from the hash and are held only
# natively (the receiver hash is off-stack), so they must be pinned
# across that alloc. Under STRESS_GC an unrooted pair was swept ->
# dangling result slots -> ICE. Object keys+values exercise it.
class P
  def initialize(i); @i = i; end
  def i; @i; end
end
h = {}
30.times { |n| h[P.new(n)] = P.new(n + 100) }
out = []
until h.empty?
  k, v = h.shift
  out << [k.i, v.i]
end
p out
p h.size
p out.length
