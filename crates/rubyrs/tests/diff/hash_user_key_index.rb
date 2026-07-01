# Hash/Set with keys that override Ruby-level `hash`/`eql?` (e.g. AST nodes,
# Parser::Source::Range). rubyrs now buckets them by their Ruby `#hash`
# (user_index) instead of a linear eql? scan — O(1)-amortized, not O(n²).
# This pins the CORRECTNESS of that path (the perf is separate); it must
# match CRuby exactly across insert/dedup/find/delete/update/iteration.

class Key
  attr_reader :a, :b
  def initialize(a, b); @a = a; @b = b; end
  def hash; [@a, @b].hash; end
  def eql?(o); o.is_a?(Key) && o.a == @a && o.b == @b; end
  alias == eql?
end

# --- Hash: dedup by value, update in place, find by equal-value key
h = {}
h[Key.new(1, 2)] = "x"
h[Key.new(3, 4)] = "y"
h[Key.new(1, 2)] = "z"                 # same key by eql? → updates
puts h.size                            # 2
puts h[Key.new(1, 2)]                  # z
puts h[Key.new(3, 4)]                  # y
puts h[Key.new(9, 9)].inspect          # nil
puts h.key?(Key.new(3, 4))             # true
puts h.key?(Key.new(5, 5))             # false

# --- delete then re-find (index must invalidate)
puts h.delete(Key.new(1, 2))           # z
puts h.size                            # 1
puts h[Key.new(1, 2)].inspect          # nil
h[Key.new(1, 2)] = "back"              # re-insert after delete
puts h[Key.new(1, 2)]                  # back

# --- Set (built on Hash): add?/include?/size
require "set"
s = Set.new
100.times { |i| s << Key.new(i % 10, 0) }   # only 10 distinct by eql?
puts s.size                            # 10
puts s.include?(Key.new(5, 0))         # true
puts s.include?(Key.new(5, 1))         # false
puts s.add?(Key.new(5, 0)).inspect     # nil (present)
puts s.add?(Key.new(99, 99)).class     # Set (new)
puts s.size                            # 11

# --- hash-collision keys (same #hash, different eql?) still separate
class Fixed
  def initialize(n); @n = n; end
  def hash; 42; end                    # everyone collides
  def eql?(o); o.is_a?(Fixed) && o.instance_variable_get(:@n) == @n; end
  alias == eql?
end
g = {}
g[Fixed.new(1)] = "one"; g[Fixed.new(2)] = "two"; g[Fixed.new(1)] = "ONE"
puts g.size                            # 2 (same bucket, distinct by eql?)
puts g[Fixed.new(1)]                   # ONE
puts g[Fixed.new(2)]                   # two

# --- iteration order preserved (insertion order)
o = {}
[[1,1],[2,2],[3,3]].each { |a, b| o[Key.new(a, b)] = a }
puts o.keys.map(&:a).inspect           # [1, 2, 3]
puts o.values.inspect                  # [1, 2, 3]
# (NOTE: Hash#merge over user-hash keys is a SEPARATE pre-existing gap —
#  merge's dedup uses native ruby_eql, not the key's eql? — tracked apart
#  from this index fix, which covers []/[]=/key?/delete/Set#add?.)
