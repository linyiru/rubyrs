# Enumerator.new { |y| ... } — the generator/yielder form. The yielder's
# `<<` and `yield` forward to the consumer's iteration block; rubyrs runs
# the generator EAGERLY (no lazy next/peek). Early-exit helpers use
# throw/catch since `break` can't cross the generator proc.
e = Enumerator.new do |y|
  y << 1
  y << 2
  y.yield 3
end
p e.class                       # Enumerator
p e.to_a                        # [1, 2, 3]
out = []; e.each { |x| out << x * 10 }; p out   # [10, 20, 30]
p e.map { |x| x + 100 }         # [101, 102, 103]
p e.select { |x| x > 1 }        # [2, 3]
p e.reject { |x| x > 1 }        # [1]
p e.first                       # 1
p e.first(2)                    # [1, 2]
p e.first(0)                    # []
p e.include?(2)                 # true
p e.include?(9)                 # false
p e.count                       # 3

# `<<` chains (returns self).
e2 = Enumerator.new { |y| y << :a << :b << :c }
p e2.to_a                       # [:a, :b, :c]

# Leading size arg is accepted and ignored.
e3 = Enumerator.new(3) { |y| y << 1; y << 2 }
p e3.to_a                       # [1, 2]

# Multi-value yield; consumer block destructures.
e4 = Enumerator.new { |y| y.yield(1, 2); y.yield(3, 4) }
pairs = []; e4.each { |a, b| pairs << [a, b] }; p pairs   # [[1, 2], [3, 4]]

# The enum_for(:meth, *args) form is unaffected.
class Seq
  def initialize(*xs); @xs = xs; end
  def each; return enum_for(:each) unless block_given?; @xs.each { |x| yield x }; end
end
p Seq.new(7, 8, 9).each.to_a    # [7, 8, 9]
p Seq.new(7, 8, 9).each.map { |x| x - 1 }  # [6, 7, 8]
