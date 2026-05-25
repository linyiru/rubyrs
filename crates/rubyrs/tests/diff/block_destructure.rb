# Block parameter destructuring `|(a, b)|` — for the common
# one-array-arg-per-iteration shape, behaves like auto-splat `|a, b|`
# but is explicit about the destructure intent.

# Array of pairs.
pairs = [[1, 2], [3, 4], [5, 6]]
pairs.each { |(a, b)| puts "#{a}+#{b}=#{a + b}" }

# Hash#each yields [k, v] arrays — destructure pair.
h = { one: 1, two: 2, three: 3 }
h.each { |(k, v)| puts "#{k}->#{v}" }

# Three-element destructure.
[[10, 20, 30]].each { |(a, b, c)| puts "#{a},#{b},#{c}" }

# Short array — missing slots become nil, matching CRuby semantics.
[[1]].each { |(a, b)| puts "#{a.inspect},#{b.inspect}" }

# Long array — extras discarded.
[[1, 2, 3, 4]].each { |(a, b)| puts "#{a},#{b}" }

# With map: destructured block result is whatever the body returns.
result = [[1, 2], [3, 4]].map { |(a, b)| a * b }
puts result.inspect

# Inside a method body — pure-destructure block (one-arg-per-iter).
class PairSum
  def total(items)
    sum = 0
    items.each { |(a, b)| sum = sum + a + b }
    sum
  end
end
puts PairSum.new.total([[1, 2], [3, 4], [5, 6]])  # 21

# Behaves the same as `|a, b|` for the simple pair case.
[[1, 2]].each { |a, b| puts "non-destruct: #{a}, #{b}" }
[[1, 2]].each { |(a, b)| puts "destruct: #{a}, #{b}" }
