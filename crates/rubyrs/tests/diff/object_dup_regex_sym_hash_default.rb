# Task-2 batch surface: Regexp#===(Symbol) + $~ publication,
# Hash#default=, Object#dup shallow semantics. The RNG family
# (shuffle/sample/srand) is property-tested in t2 probes, not
# diffed — sequences are documented Mulberry32 divergence.

p(/^test_/ === :test_a)
p(/^test_/ === :helper)
p [:test_a, :helper, :test_b].grep(/^test_/)
p(/e(s)t/ === :test_a)
p $~[1]

h = {}
h.default = 42
p h[:missing]
p h.default
h.default = nil
p h[:missing]
db = Hash.new { |hh, k| "blk" }
db.default = 7
p db[:miss]

class DupProbe
  attr_accessor :x
  def initialize
    @x = [1, 2]
  end
end
d = DupProbe.new
d2 = d.dup
d2.x << 3
p d2.class
p d.x
p d.equal?(d2)
d.freeze
p d.dup.frozen?
class DupCustom
  def dup
    "custom"
  end
end
p DupCustom.new.dup
