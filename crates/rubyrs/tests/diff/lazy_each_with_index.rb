# Enumerator::Lazy#each_with_index yields [element, index] pairs
# lazily (like with_index(0)), so it works over an infinite source.
p (1..Float::INFINITY).lazy.each_with_index.first(3)
p (1..5).lazy.each_with_index.to_a
p (1..Float::INFINITY).lazy.each_with_index.map { |x, i| x * i }.first(3)
p (1..Float::INFINITY).lazy.select(&:even?).each_with_index.first(3)
p ["a", "b", "c"].lazy.each_with_index.to_a
p (1..Float::INFINITY).lazy.map { |x| x * 10 }.each_with_index.first(2)
