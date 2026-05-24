# Array#sum
puts [1, 2, 3, 4, 5].sum
puts [].sum
puts [1, 2, 3].sum(10)

# Array#min / #max
puts [3, 1, 4, 1, 5, 9, 2, 6].min
puts [3, 1, 4, 1, 5, 9, 2, 6].max
puts [].min.nil?
puts [].max.nil?
puts ["banana", "apple", "cherry"].min
puts ["banana", "apple", "cherry"].max

# Array#sort returns a new sorted Array, source is untouched
src = [3, 1, 4, 1, 5, 9, 2, 6]
sorted = src.sort
puts sorted[0]
puts sorted[-1]
puts sorted.length
puts src[0]      # original first element still 3
puts src.length  # original untouched

# Array#count
puts [1, 2, 3, 4, 5].count
puts [1, 2, 2, 3, 2].count(2)
puts [1, 2, 3, 4, 5].count { |x| x > 2 }
puts [1, 2, 3].count { |_x| false }

# Array#inject / #reduce — block form, no init
puts [1, 2, 3, 4, 5].inject { |a, b| a + b }
puts [1, 2, 3, 4].reduce { |a, b| a * b }
puts [].inject { |a, b| a + b }.nil?

# Array#inject — block form with init
puts [1, 2, 3].inject(100) { |a, b| a + b }
puts [1, 2, 3].inject("") { |a, b| a + b.to_s }

# Array#inject — symbol form
puts [1, 2, 3, 4, 5].inject(:+)
puts [1, 2, 3, 4].inject(:*)
puts [10, 1, 2].inject(:-)

# --- Range aggregation ---
puts (1..10).sum
puts (1..10).sum(100)
puts (1...10).sum            # 1..9 = 45
puts (5..2).sum              # empty: 0
puts (1..100).sum            # 5050

puts (1..5).inject { |a, b| a + b }
puts (1..5).inject(10) { |a, b| a + b }
puts (1..5).inject(:+)
puts (1..4).inject(:*)

puts (1..10).count { |n| n % 2 == 0 }
puts (1..10).count { |n| n > 100 }

# Chained aggregation idioms
puts (1..10).select { |n| n % 2 == 0 }.sum   # 30
puts [1, 2, 3, 4, 5].map { |n| n * n }.inject(:+)  # 55

# Aggregation inside a class method
class Stats
  def initialize(values)
    @values = values
  end
  def total
    @values.inject(0) { |a, b| a + b }
  end
  def average
    total / @values.length
  end
end

s = Stats.new([10, 20, 30, 40])
puts s.total
puts s.average
