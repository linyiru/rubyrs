# `include Enumerable` + `def each` gains the whole Enumerable API, all
# built on #each (CRuby parity). liquid's InputIterator relies on this.
class Coll
  include Enumerable
  def initialize(*items); @items = items; end
  def each; @items.each { |i| yield i }; end
end
c = Coll.new(3, 1, 2, 1)
p c.to_a                          # [3,1,2,1]
p c.map { |x| x * 2 }             # [6,2,4,2]
p c.flat_map { |x| [x, -x] }      # [3,-3,1,-1,2,-2,1,-1]
p c.select { |x| x > 1 }          # [3,2]
p c.reject { |x| x > 1 }          # [1,1]
p c.filter_map { |x| x * 10 if x > 1 }  # [30,20]
p c.find { |x| x > 1 }            # 3
p c.find_index { |x| x == 2 }     # 2
p c.find_index(1)                 # 1
p c.sort                          # [1,1,2,3]
p c.sort_by { |x| -x }            # [3,2,1,1]
p c.min                           # 1
p c.max                           # 3
p c.min_by { |x| -x }             # 3
p c.max_by { |x| -x }             # 1
p c.reduce(:+)                    # 7
p c.reduce(10) { |a,b| a+b }      # 17
p c.inject(:*)                    # 6
p c.sum                           # 7
p c.sum(100)                      # 107
p c.count                         # 4
p c.count(1)                      # 2
p c.count { |x| x > 1 }           # 2
p c.include?(2)                   # true
p c.member?(9)                    # false
p c.group_by { |x| x.odd? }       # {true=>[3,1,1],false=>[2]}
p c.partition { |x| x > 1 }       # [[3,2],[1,1]]
p c.all? { |x| x > 0 }            # true
p c.any? { |x| x > 2 }            # true
p c.none? { |x| x > 9 }           # true
p c.one? { |x| x == 2 }           # true
p c.first                         # 3
p c.first(2)                      # [3,1]
p c.take(2)                       # [3,1]
p c.drop(2)                       # [2,1]
p c.take_while { |x| x > 1 }      # [3]
p c.drop_while { |x| x > 1 }      # [1,2,1]
p c.uniq                          # [3,1,2]
p c.tally                         # {3=>1,1=>2,2=>1}
p c.each_with_index.to_a          # [[3,0],[1,1],[2,2],[1,3]]
p c.each_with_object([]) { |x,a| a << x*x }  # [9,1,4,1]
p c.to_h { |x| [x, x*x] }         # {3=>9,1=>1,2=>4}
p c.lazy.map { |x| x*2 }.first(2) # [6,2]
# no-block forms return an Enumerator
p c.map.class.to_s                # "Enumerator"
p c.select.with_index { |x,i| i.even? }  # [3,2]
