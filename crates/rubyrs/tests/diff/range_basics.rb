# Inclusive and exclusive
puts (1..5).first
puts (1..5).last
puts (1...5).last
puts (1..5).size
puts (1...5).size
puts (1..5).to_a.length
puts (1..5).to_a[0]
puts (1..5).to_a[4]
puts (1...5).to_a[3]
puts (1..5).include?(3)
puts (1..5).include?(5)
puts (1...5).include?(5)
puts (1..5).include?(0)
puts (1..5).exclude_end?
puts (1...5).exclude_end?

# Iterate
sum = 0
(1..10).each { |n| sum = sum + n }
puts sum

# Exclusive iterate
sum2 = 0
(1...10).each { |n| sum2 = sum2 + n }
puts sum2

# Map over a range
squares = (1..5).map { |n| n * n }
puts squares.length
puts squares[0]
puts squares[4]

# Range as receiver of times-like idiom
seen = []
(0..2).each { |i| seen << i }
puts seen.length
puts seen[0]
puts seen[2]

# Empty / inverted range — iteration count is 0
empty_sum = 0
(5..2).each { |n| empty_sum = empty_sum + n }
puts empty_sum
puts (5..2).to_a.length

# Range methods in a class context
class Stepper
  def initialize(stop)
    @stop = stop
  end
  def total
    s = 0
    (1..@stop).each { |n| s = s + n }
    s
  end
end

puts Stepper.new(100).total
