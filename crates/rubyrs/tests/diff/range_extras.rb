# Endless / beginless Range + Range#step.
# Endless: (5..)   — first/last as expected, can take first(n).
# Beginless: (..5) — cover?/include? against an upper bound.
# Step: (1..10).step(n).to_a — step-arithmetic enumeration.

# Endless range basics.
r1 = (5..)
p r1.begin
p r1.end
p r1.first(3)
p r1.first(0)
p r1.include?(10)
p r1.include?(3)
p r1.cover?(100)
p r1.cover?(2)

# Beginless range basics.
r2 = (..5)
p r2.begin
p r2.end
p r2.cover?(3)
p r2.cover?(5)
p r2.cover?(6)
p r2.cover?(-100)
p r2.include?(3)
p r2.include?(6)

# Beginless exclusive.
r2x = (...5)
p r2x.cover?(4)
p r2x.cover?(5)

# String slicing with endless range.
s = "hello world"
p s[6..]
p s[..4]
p s[6..-1]
p s[6..100]    # over-long clamps

# Empty slice when start past end.
p s[20..]

# Step on a closed range.
p (1..10).step(2).to_a
p (0..20).step(5).to_a
p (1..6).step(3).to_a

# Step that exactly hits the end.
p (0..10).step(5).to_a

# Step that doesn't reach end.
p (0..7).step(3).to_a

# Exclusive Range with step.
p (0...10).step(3).to_a

# Step inside a class method.
class StepGen
  def initialize(from, to, by)
    @from = from
    @to = to
    @by = by
  end
  def values
    (@from..@to).step(@by).to_a
  end
end

p StepGen.new(0, 20, 4).values

# Step inside an iterator.
sums = []
(1..10).step(2).to_a.each { |n| sums << n * n }
p sums

# Step with 1 is the whole range.
p (1..5).step(1).to_a

# Zero step raises ArgumentError in CRuby.
begin
  (1..5).step(0).to_a
rescue ArgumentError
  puts "caught zero step"
end
