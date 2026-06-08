# An enumerator over a multi-value (pair) yielder preserves both values
# through to_a/map/select/first/include? now that the helpers use
# `|*x|` + `yield(*x)` (was: collapsed to the first value).
class Pairs
  def each_pair
    return enum_for(:each_pair) unless block_given?
    yield :a, 1
    yield :b, 2
    yield :c, 3
  end
end
e = Pairs.new.each_pair
p e.to_a                          # [[:a,1],[:b,2],[:c,3]]
p e.map { |k, v| "#{k}=#{v}" }    # ["a=1","b=2","c=3"]
p e.select { |k, v| v > 1 }       # [[:b,2],[:c,3]]
p e.reject { |k, v| v > 1 }       # [[:a,1]]
p e.count                         # 3
p e.first(2)                      # [[:a,1],[:b,2]]
p e.include?([:b, 2])             # true
e.with_index(1) { |pair, i| p [i, pair] }  # [1,[:a,1]] ...
# single-value enumerator unaffected
class Nums
  def go; return enum_for(:go) unless block_given?; yield 7; yield 8; end
end
p Nums.new.go.to_a                # [7, 8]
p Nums.new.go.map { |x| x * 2 }   # [14, 16]
