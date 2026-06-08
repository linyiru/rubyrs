# Object#enum_for / #to_enum (the `enum_for(:meth, *args)` deferred-
# iteration form) + a minimal Enumerator. The classic guard is
# `return enum_for(:meth) unless block_given?` at the top of an
# iterator — rouge's lexer/formatter pipeline relies on it
# (`lex(string)` without a block returns `enum_for(:lex, string)`,
# later driven via `tokens.each { … }`).

class Seq
  def initialize(*items); @items = items; end
  # single-value iterator
  def each
    return enum_for(:each) unless block_given?
    @items.each { |x| yield x }
  end
  # iterator taking an arg, yielding pairs
  def each_tagged(tag)
    return enum_for(:each_tagged, tag) unless block_given?
    @items.each_with_index { |x, i| yield tag, x }
  end
end

s = Seq.new(10, 20, 30)

# enum_for without a block returns an Enumerator.
e = s.each
p e.class                       # Enumerator
p e.is_a?(Enumerable)           # true

# Driving the enumerator re-invokes the captured method.
acc = []
e.each { |x| acc << x }
p acc                           # [10, 20, 30]

# Enumerable-style helpers (single-value).
p s.each.to_a                   # [10, 20, 30]
p s.each.map { |x| x + 1 }      # [11, 21, 31]
p s.each.select { |x| x > 15 }  # [20, 30]
p s.each.reject { |x| x > 15 }  # [10]
p s.each.count                  # 3
p s.each.first                  # 10
p s.each.first(2)               # [10, 20]
p s.each.include?(20)           # true

# enum_for carries args; the consumer block sees the forwarded yields.
pairs = []
s.each_tagged(:t).each { |tag, v| pairs << "#{tag}:#{v}" }
p pairs                         # ["t:10", "t:20", "t:30"]

# to_enum is an alias.
p s.to_enum(:each).to_a         # [10, 20, 30]

# with_index drives index alongside value.
idx = []
s.each.with_index(0) { |x, i| idx << [i, x] }
p idx                           # [[0, 10], [1, 20], [2, 30]]
