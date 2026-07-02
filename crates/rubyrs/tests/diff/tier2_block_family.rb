# Tier-2 wave-5 BLOCK battery (ADR 0037): compiled block bodies served at the
# invoke_block sites (the Op::Yield arm, the step_block iter drivers,
# Proc#call) must behave byte-identically to the interpreter across every
# block-semantics edge: value-carrying break through compiled and interpreted
# yielders, next-with-value, nested yields, captured-outer-local REBINDING
# from a compiled block (shared-binding contract), per-invocation isolation on
# the copy path, $~ transparency, ensure-in-the-yielder on break, &block
# forwarding, proc-vs-lambda arity, autosplat/rest/kw/block-param binding
# (done by the interpreter's own binder before native entry), redo,
# Enumerator/StopIteration. Warm loops run past the adaptive compile
# threshold (base 1024 + 16/op) so under RUBYRS_JIT_TIER2=1 the hot blocks
# are native when the behavior checks run; RUBYRS_JIT_TIER2_THRESHOLD=1
# compiles everything on first entry.

# -- compiled block REBINDING captured outer locals (share-direct path) +
#    each_with_index/each_with_object shapes
def counter_sum(arr)
  total = 0
  idx = 0
  arr.each { |x| total += x * (idx += 1) }
  [total, idx]
end
arr = (1..40).to_a
acc = 0
1500.times { acc += counter_sum(arr).sum }
puts acc

ewi = 0
1500.times { arr.each_with_index { |x, i| ewi += x + i } }
puts ewi
ewo = arr.each_with_object([]) { |x, memo| memo << x * 2 if x.even? }
1500.times { arr.each_with_object([]) { |x, memo| memo << x * 2 if x.even? } }
puts ewo.sum

# -- Hash |k, v| two-arg binding (invoke_block2 path)
h = { a: 1, b: 2, c: 3 }
hs = 0
2000.times { h.each { |k, v| hs += k.size + v } }
puts hs

# -- autosplat: pair rows into |a, b| and |a, *b|; rest-only |*a| keeps whole
pairs = [[1, 2], [3, 4], [5, 6]]
ps = 0
2000.times { pairs.each { |a, b| ps += a * 10 + b } }
puts ps
rest_shape = []
pairs.each { |a, *b| rest_shape << [a, b] }
puts rest_shape.inspect
whole = []
[[7, 8]].each { |*a| whole << a }
puts whole.inspect

# -- yield: plain, multi-arg drop, splat (ApplyYield), nested-block yield
def yielder_p(v)
  yield v
end
def y_multi
  yield 1, 2, 3
end
def y_splat(a)
  yield(*a)
end
def each_pair2
  [1, 2].each { |a| [3, 4].each { |b| yield a, b } }
end
ys = 0
2000.times { |i| ys += yielder_p(i) { |x| x + 1 } }
puts ys
puts(y_multi { |a, b| [a, b] }.inspect)
2000.times { ys += y_splat([2, 3]) { |a, b| a * b } }
puts ys
np = []
each_pair2 { |a, b| np << a * 10 + b }
puts np.inspect

# -- value-carrying break through a COMPILED yielder (warm first, then break)
puts(yielder_p(1) { break 42 })
# break through TWO yielders: the break belongs to outer_y's caller block
def outer_y
  yielder_p(7) { |v| yield v }
end
puts(outer_y { |v| v * 2 })
puts(outer_y { break 99 })
# break out of a Rust iter driver below a compiled block
def find_big(arr)
  r = arr.each { |x| break x * 100 if x > 38 }
  r
end
1500.times { find_big(arr) }
puts find_big(arr)

# -- next with value (hot block)
nx = 0
2000.times { nx = arr.map { |x| next x * 10 if x.odd?; x }.sum }
puts nx

# -- non-local return from a block below a compiled method frame
def find_first(arr)
  arr.each { |x| return x if x > 20 }
  :none
end
1500.times { find_first(arr) }
puts find_first(arr)

# -- ensure in the yielder runs on break (yielder declines tier-2, block hot)
$ens = 0
def with_ensure
  yield 5
  :normal
ensure
  $ens += 1
end
we = 0
2000.times { we += with_ensure { |x| x * 2 } == :normal ? 1 : 0 }
puts we
puts(with_ensure { break :br })
puts $ens

# -- $~ transparency: a match inside a block is visible to the method scope;
#    a callee's match never leaks into the caller
def match_in_block(strs)
  strs.each { |s| s =~ /(\d+)/ }
  $1
end
"outer 777" =~ /(\d+)/
1500.times { match_in_block(["a1", "b22"]) }
puts match_in_block(["a1", "b22"])
puts $1

# -- &block forwarding through a method into an iter driver
def fwd(&b)
  [1, 2, 3].each(&b)
end
fs = 0
2000.times { fwd { |x| fs += x } }
puts fs

# -- Proc#call vs yield arity: procs lenient, lambdas strict
p2 = proc { |a, b| [a, b] }
2000.times { p2.call(1, 2) }
puts p2.call(1).inspect
puts p2.call(1, 2, 3).inspect
l2 = ->(a, b) { [a, b] }
2000.times { l2.call(1, 2) }
begin
  l2.call(1)
rescue ArgumentError => e
  puts e.class
end
puts(y_multi { |a, b| [a, b] }.inspect)

# -- kw and block-param blocks (interpreter binder + compiled body)
pk = proc { |a, k: 10| a + k }
2000.times { pk.call(1, k: 5) }
puts pk.call(1)
puts pk.call(1, k: 5)
bt = proc { |x, &b| b ? b.call(x) : x }
2000.times { bt.call(2) }
puts bt.call(2)
puts(bt.call(2) { |v| v * 3 })

# -- numbered params (bind through the same param slots)
np1 = 0
2000.times { [1, 2, 3].each { np1 += _1 * 2 } }
puts np1
np2 = 0
2000.times { { a: 1, b: 2 }.each { np2 += _2 } }
puts np2

# -- escaped proc rebinding its (dead) creator's local through the chain
def make_counter
  n = 0
  -> { n += 1 }
end
inc = make_counter
3000.times { inc.call }
puts inc.call

# -- per-invocation isolation on the copy path (block creates inner lambdas)
def make_doublers(xs)
  procs = []
  xs.each { |x| procs << -> { x * 2 } }
  procs.map(&:call)
end
1500.times { make_doublers([1, 2, 3]) }
puts make_doublers([1, 2, 3]).inspect

# -- redo: re-run the current iteration once per element (hot, compiled jump)
big = (1..600).to_a
seen = 0
done = {}
big.each do |x|
  seen += 1
  unless done[x]
    done[x] = true
    redo
  end
end
puts seen

# -- Enumerator + StopIteration smoke
e = [10, 20, 30].each
vals = []
loop { vals << e.next }
puts vals.inspect
enum_sum = 0
1500.times { enum_sum += [1, 2, 3].each_slice(2).map(&:sum).max }
puts enum_sum

# -- while-loop + next/break INSIDE a hot block (native loop edges)
wl = 0
1200.times do |i|
  j = 0
  while j < 5
    j += 1
    next if j == 2
    break if j == 4
    wl += 1
  end
end
puts wl
