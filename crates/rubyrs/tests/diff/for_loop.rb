# `for x in coll … end` desugars to `coll.each { |x| … }`. (CRuby's
# `for` leaks the loop var to the surrounding scope; rubyrs scopes it
# to the block — a documented divergence, so we don't assert the
# post-loop variable.) Discovery: P3 Jekyll spike — kramdown's
# html.rb uses `for … in … end` at load time.
total = 0
for x in [1, 2, 3]
  total += x
end
p total

# multi-target destructuring
acc = []
for a, b in [[1, 2], [3, 4], [5, 6]]
  acc << (a * b)
end
p acc

# over a Range
sum = 0
for i in 1..5
  sum += i
end
p sum

# over String#chars, building a result
out = ""
for ch in "abc".chars
  out << ch.upcase
end
p out

# over a Hash (yields [k, v] pairs)
pairs = []
for k, v in { a: 1, b: 2 }
  pairs << "#{k}=#{v}"
end
p pairs

# for with a body that uses next
collected = []
for n in [1, 2, 3, 4]
  next if n.even?
  collected << n
end
p collected
