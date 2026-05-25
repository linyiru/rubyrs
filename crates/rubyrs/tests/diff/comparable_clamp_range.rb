# Comparable#clamp Range form — `x.clamp(lo..hi)`.
# (Numeric primitives don't include Comparable in the subset
# yet, so the fixture exercises a user class that mixes
# Comparable in.)

class Score
  include Comparable
  attr_reader :n
  def initialize(n); @n = n; end
  def <=>(o); @n <=> o.n; end
  def inspect; "Score(#{@n})"; end
end

lo  = Score.new(1)
hi  = Score.new(10)

# Inside range — receiver returned.
puts Score.new(5).clamp(lo..hi).inspect    # Score(5)
puts Score.new(5).clamp(lo, hi).inspect    # Score(5) — 2-arg still works

# Below low — low returned.
puts Score.new(-3).clamp(lo..hi).inspect   # Score(1)
puts Score.new(-3).clamp(lo, hi).inspect   # Score(1)

# Above high — high returned.
puts Score.new(99).clamp(lo..hi).inspect   # Score(10)
puts Score.new(99).clamp(lo, hi).inspect   # Score(10)

# Boundary inclusion.
puts Score.new(1).clamp(lo..hi).inspect    # Score(1)
puts Score.new(10).clamp(lo..hi).inspect   # Score(10)

# Wrong arg count surfaces ArgumentError.
begin
  Score.new(5).clamp
rescue ArgumentError => e
  puts "0-arg: caught"
end

begin
  Score.new(5).clamp(lo, hi, Score.new(99))
rescue ArgumentError => e
  puts "3-arg: caught"
end
