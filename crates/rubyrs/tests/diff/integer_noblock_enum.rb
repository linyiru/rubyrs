# No-block Integer iterators (#times/#upto/#downto) return an Enumerator,
# so the common drives work: to_a, map, select, first, with chaining.
p 5.times.to_a
p 5.times.map { |i| i * i }
p 3.times.select(&:even?)
p 0.times.to_a
p 5.times.first(3)
p 1.upto(5).to_a
p 1.upto(5).map { |x| x * 2 }
p 1.upto(3.0).to_a
p 5.downto(1).to_a
p 5.downto(1).select(&:odd?)
p 5.respond_to?(:times)
p 1.respond_to?(:upto)
# block forms still return the receiver / honour break
r = []; 3.times { |i| r << i }; p r
p(5.times { |i| break i * 100 if i == 2 })
