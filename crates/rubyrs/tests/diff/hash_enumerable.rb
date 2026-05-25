# Hash Enumerable completion — sort, sort_by, min_by, max_by,
# group_by. Each yields (k, v) as two block args (matching CRuby).

h = {"c" => 3, "a" => 1, "b" => 2}

# sort (no block) — Array of [k, v] pairs ordered by key via <=>.
puts h.sort.inspect

# sort_by by value.
puts h.sort_by { |k, v| v }.inspect

# sort_by descending via negative-key trick.
puts h.sort_by { |k, v| -v }.inspect

# sort_by by key, even though sort would do the same.
puts h.sort_by { |k, v| k }.inspect

# min_by / max_by return the winning [k, v] pair.
puts h.min_by { |k, v| v }.inspect
puts h.max_by { |k, v| v }.inspect
puts h.min_by { |k, v| -v }.inspect

# group_by — bucket pairs.
puts h.group_by { |k, v| v.even? ? :even : :odd }.inspect

# Empty Hash edge cases.
puts({}.sort.inspect)
puts({}.sort_by { |k, v| v }.inspect)
puts({}.min_by { |k, v| v }.inspect)
puts({}.max_by { |k, v| v }.inspect)
puts({}.group_by { |k, v| k }.inspect)

# Single-key Hash.
single = {"only" => 42}
puts single.sort.inspect
puts single.min_by { |k, v| v }.inspect
puts single.max_by { |k, v| v }.inspect

# sort_by with a user-class key — exercises user_cmp.
class Score
  include Comparable
  attr_reader :n
  def initialize(n); @n = n; end
  def <=>(o); @n <=> o.n; end
  def to_s; "S#{@n}"; end
end

scored = {"a" => Score.new(3), "b" => Score.new(1), "c" => Score.new(2)}
ordered = scored.sort_by { |k, v| v }
puts ordered.map { |pair| "#{pair[0]}=#{pair[1].to_s}" }.inspect

# group_by with String keys.
fruits = {"apple" => 1, "banana" => 2, "avocado" => 3, "blueberry" => 4}
grouped = fruits.group_by { |k, v| k.index("a") == 0 ? "a" : "b" }
puts grouped["a"].length
puts grouped["b"].length

# Counter-style: extract top 2 entries by value.
counts = {"x" => 5, "y" => 1, "z" => 9, "w" => 3}
top = counts.sort_by { |k, v| -v }.take(2)
puts top.map { |p| "#{p[0]}=#{p[1]}" }.inspect

# Chains with Hash#select then sort_by.
filtered = h.select { |k, v| v > 1 }.sort_by { |k, v| k }
puts filtered.inspect

# min_by inside a method.
class Tally
  def initialize
    @h = {}
  end
  def bump(k)
    @h[k] ||= 0
    @h[k] += 1
  end
  def least
    @h.min_by { |k, v| v }
  end
  def most
    @h.max_by { |k, v| v }
  end
end

t = Tally.new
["a", "b", "a", "c", "a", "b"].each { |x| t.bump(x) }
puts t.most.inspect
puts t.least.inspect
