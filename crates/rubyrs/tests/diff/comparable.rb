# Comparable — `include Comparable` derives `<`, `<=`, `>`, `>=`,
# `==`, `between?`, and `clamp` from a user-defined `<=>`.

class Version
  include Comparable
  attr_reader :n
  def initialize(n)
    @n = n
  end
  def <=>(other)
    @n <=> other.n
  end
  def to_s
    "v#{@n}"
  end
end

a = Version.new(3)
b = Version.new(5)
c = Version.new(3)

# Six pairwise relations.
puts a < b
puts a <= b
puts a > b
puts a >= b
puts a == b
puts a == c
puts a < c
puts a <= c
puts a > c
puts a >= c

# between? — inclusive bounds.
puts a.between?(Version.new(1), Version.new(10))
puts a.between?(Version.new(4), Version.new(10))
puts a.between?(Version.new(3), Version.new(3))

# clamp — pin into [lo, hi].
puts Version.new(20).clamp(Version.new(0), Version.new(10)).to_s
puts Version.new(-5).clamp(Version.new(0), Version.new(10)).to_s
puts Version.new(5).clamp(Version.new(0), Version.new(10)).to_s
puts Version.new(0).clamp(Version.new(0), Version.new(10)).to_s
puts Version.new(10).clamp(Version.new(0), Version.new(10)).to_s

# `include` is non-destructive: a class method overrides the
# included one.
class Weighted
  include Comparable
  attr_reader :w
  def initialize(w)
    @w = w
  end
  def <=>(other)
    @w <=> other.w
  end
  def <(other)
    "custom-lt"
  end
end

puts Weighted.new(1) < Weighted.new(2)
puts Weighted.new(2) <= Weighted.new(1)

# Multiple classes can share Comparable without interference.
class Score
  include Comparable
  attr_reader :s
  def initialize(s)
    @s = s
  end
  def <=>(other)
    @s <=> other.s
  end
end

s1 = Score.new(80)
s2 = Score.new(90)
puts s1 < s2
puts s1.between?(Score.new(70), Score.new(85))

# Comparable on numeric-wrapping classes plays nicely with chains.
puts Version.new(7) > Version.new(3) && Version.new(7) < Version.new(10)

# `include Comparable` returns the class itself, as in CRuby —
# so subsequent class-body statements still see the right `self`.
class Levels
  include Comparable
  def initialize(n); @n = n; end
  def <=>(o); @n <=> o.instance_variable_get(:@n); end
end
# (no assertion — just verifying parse + method-table layout)
puts "done"
