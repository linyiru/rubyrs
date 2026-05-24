# Array filters
puts [1, 2, 3, 4, 5].select { |x| x > 2 }.length
puts [1, 2, 3, 4, 5].reject { |x| x > 2 }.length
puts [1, 2, 3, 4, 5].find { |x| x > 2 }
puts [1, 2, 3, 4, 5].find { |x| x > 99 }.nil?
puts [1, 2, 3].any? { |x| x > 2 }
puts [1, 2, 3].any? { |x| x > 99 }
puts [1, 2, 3].all? { |x| x > 0 }
puts [1, 2, 3].all? { |x| x > 2 }
puts [1, 2, 3].none? { |x| x > 5 }
puts [1, 2, 3].none? { |x| x == 2 }

# Empty-array vacuous-truth corner cases
puts [].any? { |x| true }
puts [].all? { |x| false }
puts [].none? { |x| true }

# include? (no block) on Array
puts [1, 2, 3].include?(2)
puts [1, 2, 3].include?(5)
puts ["a", "b", "c"].include?("a")
puts ["a", "b", "c"].include?("z")
puts [:a, :b, :c].include?(:b)

# detect is an alias for find
puts [1, 2, 3].detect { |x| x > 1 }
# filter is an alias for select
puts [1, 2, 3].filter { |x| x > 1 }.length

# Chaining: select then map then sum-via-each
sum = 0
[1, 2, 3, 4, 5].select { |x| x > 2 }.map { |x| x * 10 }.each { |x| sum = sum + x }
puts sum

# --- Hash filters ---
h = { a: 1, b: 2, c: 3 }

sel = h.select { |_k, v| v >= 2 }
puts sel.size
puts sel[:b]
puts sel[:c]
puts sel[:a].nil?

rej = h.reject { |_k, v| v >= 2 }
puts rej.size
puts rej[:a]

# find on a Hash returns a [key, value] pair
hit = h.find { |_k, v| v == 2 }
puts hit.length
puts hit[0]
puts hit[1]

# find with no match returns nil
puts h.find { |_k, v| v > 99 }.nil?

puts h.any? { |_k, v| v > 2 }
puts h.any? { |_k, v| v > 99 }
puts h.all? { |_k, v| v > 0 }
puts h.all? { |_k, v| v > 1 }
puts h.none? { |_k, v| v > 5 }
puts h.none? { |_k, v| v == 1 }

puts h.include?(:b)
puts h.include?(:z)
puts h.has_key?(:a)
puts h.has_key?(:z)
puts h.key?(:c)
puts h.member?(:a)

# --- Range filters ---
puts (1..10).select { |n| n % 2 == 0 }.length
puts (1..10).reject { |n| n % 2 == 0 }.length
puts (1..10).find { |n| n > 5 }
puts (1..10).find { |n| n > 99 }.nil?
puts (1..10).any? { |n| n > 5 }
puts (1..10).any? { |n| n > 99 }
puts (1..10).all? { |n| n > 0 }
puts (1..10).all? { |n| n > 5 }
puts (1..10).none? { |n| n > 100 }
puts (1..10).none? { |n| n == 5 }

# Exclusive endpoint affects iteration
puts (1...5).select { |n| n > 2 }.length     # 4 is included, 5 is not
puts (1...5).find { |n| n == 5 }.nil?

# A Range filter inside a class body
class Sieve
  def initialize(stop)
    @stop = stop
  end
  def evens_below
    (1..@stop).select { |n| n % 2 == 0 }
  end
end

s = Sieve.new(10)
arr = s.evens_below
puts arr.length
puts arr[0]
puts arr[4]
