# Enumerator::Lazy#zip — pair with same-index elements (nil past end);
# stays lazy on the SOURCE (infinite source + first works).
p [1, 2, 3].lazy.zip([4, 5, 6]).to_a
p [1, 2, 3].lazy.zip([4, 5]).to_a
p [1, 2].lazy.zip([3, 4], [5, 6]).to_a
p (1..Float::INFINITY).lazy.zip([10, 20, 30]).first(2)
p [1, 2, 3].lazy.zip([9, 8, 7]).map { |a, b| a + b }.to_a
p [1, 2].lazy.zip.to_a
p (1..6).lazy.select(&:even?).zip(%w[a b c]).to_a
p [1, 2, 3].lazy.zip([4, 5, 6]).first
