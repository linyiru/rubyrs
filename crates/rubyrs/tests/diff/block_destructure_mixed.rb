# Mixed block params — named leading param + destructure tail.
# `|head, (a, b)|` consumes 2 call-interface slots: `head` binds the
# first arg directly, the second slot anonymously receives the
# pair-Array and a compile-time prologue copies elements into `a` /
# `b`.
#
# The canonical use-case is `inject` / `reduce` with paired
# elements: the accumulator stays a Single, the element is a
# Destructure.

# inject with pair elements.
pairs = [[1, 2], [3, 4], [5, 6]]
puts pairs.inject(0) { |acc, (a, b)| acc + a + b }   # 21

# each_with_index where the element is itself a pair.
[[10, 20], [30, 40]].each_with_index do |(a, b), i|
  puts "#{i}: #{a} + #{b} = #{a + b}"
end

# Three-param mix: |head, (a, b), tail|.
[[1, [2, 3], 99], [10, [20, 30], 100]].each do |head, (a, b), tail|
  puts "#{head} / #{a},#{b} / #{tail}"
end

# Coercion path: Kernel#Array used for the destructure's anonymous
# slot — non-Array values become single-element arrays, nil becomes
# empty so unpacking yields nil for each inner name.
[1, [2, 3], nil].each do |elem|
  arr = Array(elem)
  puts arr.inspect
end

# Combined with Hash#each (which yields pair-as-Array post-F4).
h = { a: 1, b: 2 }
sum = 0
h.each { |(k, v)| sum = sum + v }
puts sum   # 3
