# Nested block-param destructure `|((a, b), c)|`. The outer
# destructure receives a 2-elem Array; the first element is itself
# an Array that the inner destructure unpacks into `a` / `b`.
# Arbitrarily deep nesting works the same way.

# 2-deep destructure.
[[[1, 2], 3], [[4, 5], 6]].each { |((a, b), c)| puts "#{a},#{b},#{c}" }

# 3-deep destructure.
[[[[10, 20], 30], 40]].each { |(((p, q), r), s)| puts "#{p}/#{q}/#{r}/#{s}" }

# Mixed nesting alongside a Single leading param.
data = [[1, [10, 20]], [2, [30, 40]]]
data.each { |id, (lo, hi)| puts "id=#{id} lo=#{lo} hi=#{hi}" }

# Nested + multiple inner Singles.
[[[1, 2, 3], "x"], [[4, 5, 6], "y"]].each do |((a, b, c), tag)|
  puts "#{tag}: #{a + b + c}"
end

# Coercion: nil leaves the inner slots nil.
[[nil, 99]].each { |((a, b), c)| p [a, b, c] }
