# Blocks created inside a block frame or a define_method body (a routing
# frame) share the creator's locals cell when their body creates no
# closure. Every shape below must behave as if each invocation had its own
# scope for params / block-locals and one shared binding for outer vars.
def y; yield; end
def y1(v); yield v; end

top = 100

# Reads and writes across three scopes from a leaf block in a lambda.
lam = -> do
  mid = 10
  3.times { |i| y { top += i; mid += i } }
  [top, mid]
end
p lam.call
p top

# A local declared in the creator AFTER the block literal.
l2 = -> do
  r = []
  2.times { |i| y1(i) { |v| r << [v, (later ||= :unset)] } }
  later = :set
  [r, later]
end
p l2.call

# Params and block-locals are fresh each invocation.
l3 = -> do
  out = []
  3.times { |i| y1(i) { |v; t| out << [v, t]; t = v * 2 } }
  out
end
p l3.call
l4 = -> { (1..3).map { |i| y { z = (z || 0) + i; z } } }
p l4.call

# Sibling blocks of one creator live at the same time.
l5 = -> do
  inner = proc { |a| b = a * 10; b }
  [1, 2].map { |x| y1(x) { |v| w = inner.call(v); [v, w] } }
end
p l5.call
l5b = -> do
  [1, 2].map do |x|
    a = proc { |pa| q = pa * 100; t = q + 1; [q, t] }
    y1(x) { |v| q = v; t = a.call(v); [v, q, t, a.call(v + 1), q] }
  end
end
p l5b.call

# The same nested block re-entered through recursion.
def rec(n, &blk)
  return [] if n == 0
  [blk.call(n)] + rec(n - 1, &blk)
end
l6 = -> do
  acc = []
  [3].each { |k| acc << rec(k) { |m| q = m * k; q } }
  acc
end
p l6.call
def reenter(n, &b) = n == 0 ? [] : [b.call(n, ->(m) { reenter(m, &b) })]
l7 = -> { [1].each { } ; r = nil; [2].each { r = reenter(2) { |v, again| s = v; [s, again.(v - 1), s] } }; r }
p l7.call

# yield / return / break / next from a nested block.
def each_yield
  [1, 2].each { |x| y { yield x * 10 } }
end
res = []
each_yield { |v| res << v }
p res
def ret_method
  [1, 2, 3].each { |x| y { return x if x == 2 } }
  :none
end
p ret_method
l8 = -> { [1, 2, 3].each { |x| y { return x * 100 if x == 2 } }; :none }
p l8.call
p([1, 2, 3].each { |x| y { break x if x == 2 } })
l9 = -> { [1, 2, 3].map { |x| y { next x + 1 } } }
p l9.call
def brk; [1, 2, 3].map { |x| y1(x) { |v| break v * 7 if v == 2; v } }; end
p brk

# Exceptions, rescue and ensure inside a nested block.
l10 = -> do
  log = []
  [1, 2].each do |x|
    y do
      begin
        raise "e#{x}" if x == 2
        log << x
      rescue => e
        log << e.message
      ensure
        log << :ens
      end
    end
  end
  log
end
p l10.call

# An escaped nested proc keeps its creator's binding alive.
makers = 2.times.map do |i|
  x = i * 10
  pr = nil
  y { pr = proc { x += 1 } }
  pr
end
p makers.map { |m| [m.call, m.call] }

# define_method bodies are routing creators too.
class DM
  base = 5
  define_method(:dm) do |n|
    sum = 0
    n.times { |i| y1(i) { |v| sum += v + base } }
    sum
  end
  def y1(v) = yield(v)
end
p DM.new.dm(3)

# A deep chain: method -> block -> block -> leaf.
def deep
  a = 1
  [2].each do |b|
    [3].each do |c|
      [4].each { |d| a += b + c + d }
      y { c += 1 }
      a += c
    end
  end
  a
end
p deep

# Iteration drivers rebinding the same frame.
l11 = -> do
  total = 0
  r = [1, 2, 3].each_with_object([]) { |x, acc| acc << y1(x) { |v| t = v * 2; total += t; t } }
  [r, total]
end
p l11.call
p((-> { h = {}; { a: 1, b: 2 }.each { |k, v| y { h[v] = k } }; h }).call)

# binding / local_variables from a nested block.
l12 = -> do
  outer_v = 1
  [2].map { |x| y { inner_v = 3; [defined?(outer_v), defined?(inner_v), outer_v + x + inner_v] } }
end
p l12.call
l13 = -> { k = 1; v = :outer; r = [2].map { |x| y1(x) { |v| k += v; v } }; [r, k, v] }
p l13.call

# Lambdas stay on the copy path.
l14 = -> { [1, 2].map { |x| y1(x, &->(v) { v + 1 }) } }
p l14.call
