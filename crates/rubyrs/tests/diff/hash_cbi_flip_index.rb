# compare_by_identity flip on a LIVE-indexed hash: the flip changes
# lookup semantics, so any key index built beforehand is stale — in
# particular a user-hash/eql? pair merged in through the user-index
# path is ABSENT from the identity index, and a post-flip lookup
# consulting that stale index would miss it. The flip must invalidate
# both indexes (rebuilt lazily). (Adversarial-verifier probe
# 22_cbi_flip_min, 2026-07.)

class K
  attr_reader :v
  def initialize(v) = @v = v
  def hash = @v.hash
  def eql?(o) = o.is_a?(K) && o.v == @v
  def inspect = "K(#{@v})"
end

h = {}
16.times { |i| h[i] = i }   # cross the identity-index threshold
h[99]                        # force the index build via a lookup
k = K.new(1)
h.merge!({ k => :m })
h.compare_by_identity
p h[k]
p h.key?(k)
p h.size

# plain keys stay findable across the flip too (index rebuilt)
h2 = {}
20.times { |i| h2["k#{i}"] = i }
h2["k5"]
h2.compare_by_identity
p h2.size
p h2.value?(5)

# delete after flip of a user-key entry inserted before it
h3 = {}
16.times { |i| h3[i] = i }
kk = K.new(7)
h3[kk] = :x
h3[0]
h3.compare_by_identity
p h3.delete(kk)
p h3.size
