# closure write-through to outer scope (non-capturing block)
counter = 0
[1,2,3].each { |i| counter += i }
puts counter  # 6

# block-body locals reset per iteration
out = []
[1,2,3].each { |x| y ||= 0; y += x; out << y }
p out  # [1,2,3] (y resets each iter)

# capture isolation (CAPTURING block — must take copy path)
procs = [:a,:b,:c].map { |s| -> { s } }
p procs.map(&:call)  # [:a,:b,:c]

# nested non-capturing blocks
total = 0
[[1,2],[3,4]].each { |pair| pair.each { |n| total += n } }
puts total  # 10

# re-entrancy: same block proc called recursively
fac = nil
fac = ->(n) { n <= 1 ? 1 : n * fac.call(n-1) }
puts fac.call(5)  # 120

# re-entrant via yield in recursive method
def walk(n, &blk)
  blk.call(n)
  walk(n-1, &blk) if n > 0
end
acc = []
walk(3) { |v| acc << v }
p acc  # [3,2,1,0]

# block with many outer locals (the copy-cost target)
def heavy
  a=1;b=2;c=3;d=4;e=5;f=6;g=7;h=8
  sum = 0
  [10,20,30].each { |x| sum += x + a + b }
  sum
end
puts heavy  # 60 + 3*(1+2) = 69

# map building
sq = [1,2,3,4].map { |n| n*n }
p sq  # [1,4,9,16]

# select/reject
p [1,2,3,4,5].select { |n| n.even? }  # [2,4]

# Hash#each (invoke_block2 path)
h = {a: 1, b: 2}
res = []
h.each { |k,v| res << "#{k}=#{v}" }
p res  # ["a=1","b=2"]

# same-block-proto re-entrancy (shallow, debug-stack-cap safe)
flatten_sum = lambda do |arr|
  s = 0
  arr.each { |e| s += e.is_a?(Array) ? flatten_sum.call(e) : e }
  s
end
puts flatten_sum.call([1,[2,3],4])  # 10

# --- additional edge cases for the share-direct path ---

# detached proc writing outer scope (documented Tier-1 divergence is
# only for POST-pop writes; an active call must write through)
def make_adder
  total = 0
  add = ->(n) { total += n }
  add.call(5); add.call(7)
  total
end
puts make_adder  # 12

# block-local shadowing an outer var name reset each iteration
base = 100
res = []
[1,2,3].each { |x| base = x; res << base }
p res        # [1,2,3]
puts base    # 3 (outer base written through)

# while-loop re-invoking the SAME block object across iterations
def loop_same_block
  acc = []
  blk = proc { |v| acc << v * 2 }
  [1,2,3].each(&blk)
  [4,5].each(&blk)
  acc
end
p loop_same_block  # [2,4,6,8,10]

# nested each with outer accumulation (the bug case + deeper)
grid = [[1,2],[3,4],[5,6]]
s = 0
grid.each { |row| row.each { |v| s += v } }
puts s  # 21


# block with rest param (general invoke_block path)
collected = []
[1,2,3,4,5].each { |*xs| collected << xs.first }
p collected  # [1, 2, 3, 4, 5]
