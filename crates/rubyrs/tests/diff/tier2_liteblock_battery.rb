# LITE-BLOCK battery (ADR 0037 block-frame residue): frameless block
# bodies must be observably identical to framed ones. Loops run past the
# tier-2 compile threshold so the lite entries actually serve.
N = 300

# 1. capture-write from a (lite-served) block observed by the defining
#    scope AND a sibling block — writes must land in the original cells.
def cap_write(arr)
  t = 0
  arr.each { |x| t = t + x }
  arr.each { |x| t = t + 1 }
  t
end
a10 = (1..10).to_a
N.times { cap_write(a10) }
p cap_write(a10)

# 2. break through a FRAMED yielder with a value-carrying break (a block
#    containing break never lite-admits; the yielder may be compiled).
def yielder(arr)
  arr.each { |x| yield x }
  :fell_through
end
def break_through(arr)
  yielder(arr) { |x| break x * 100 if x == 7 }
end
N.times { break_through(a10) }
p break_through(a10)

# 3. next-with-value from a block (a lite block's Op::Return).
def nexts(arr)
  arr.map { |x| next x * 2 if x.odd?; x }
end
N.times { nexts(a10) }
p nexts(a10)

# 4. $~ set inside a block visible to the enclosing method (CRuby scoping:
#    blocks share the method's match data).
def match_in_block(strs)
  strs.each { |s| s =~ /(\d+)/ }
  $1
end
strs = ["a1", "b22", "c333"]
N.times { match_in_block(strs) }
p match_in_block(strs)

# 5. deep block-in-block recursion (re-entrant same-proto blocks: the
#    copy path + reentrancy handling under lite serving of inner leaves).
class Tree
  attr_reader :kids, :val
  def initialize(val, kids)
    @val = val
    @kids = kids
  end
  def each_kid
    kids.each { |k| yield k }
  end
  def total
    t = val
    each_kid { |k| t = t + k.total }
    t
  end
end
tree = Tree.new(1, (1..3).map { |i| Tree.new(i, (1..3).map { |j| Tree.new(i * j, []) }) })
N.times { tree.total }
p tree.total

# 6. redo inside a block (bounded).
def redo_block(arr)
  tries = 0
  out = []
  arr.each do |x|
    tries += 1
    if x == 2 && tries < 5
      redo
    end
    out << [x, tries]
  end
  out
end
p redo_block([1, 2, 3])

# 7. non-local return THROUGH a block from inside a method.
def nlr(arr)
  arr.each { |x| return x * 9 if x == 5 }
  :none
end
N.times { nlr(a10) }
p nlr(a10)

# 8. lambda strict arity via .call on a 1-param lambda (lite-eligible).
l = ->(x) { x + 1 }
N.times { l.call(4) }
p l.call(4)

# 9. escaped proc invoked after its defining frame popped — the
#    outer-cell routing must still hit the original binding cell.
def make_counter
  n = 0
  -> { n = n + 1 }
end
c = make_counter
N.times { c.call }
p c.call

# 10. blocks over heterogeneous values (tag-guard bails mid-loop).
def mixed_sum(arr)
  t = 0
  arr.each { |x| t = t + (x.is_a?(Integer) ? x : x.length) }
  t
end
mixed = [1, 2, "abc", 3, "de"]
N.times { mixed_sum(mixed) }
p mixed_sum(mixed)

# 11. ivar read/write from a block on the block's self.
class Counter
  def initialize
    @n = 0
  end
  def bump(arr)
    arr.each { |x| @n = @n + x }
    @n
  end
end
cnt = Counter.new
N.times { cnt.bump(a10) }
p cnt.bump(a10)

# 12. yield inside a block body (declines lite; must stay correct).
def y2(arr)
  arr.each { |x| yield x }
end
def use_y2(arr)
  acc = []
  y2(arr) { |x| acc << x * 3 }
  acc.last
end
N.times { use_y2(a10) }
p use_y2(a10)

# 13. 2-param lite serve (Hash#each) + the AUTO-SPLAT shape for a 2-param
#    block (single Array arg via Array#each — must keep the general
#    binder, NOT the 2-arg lite entry: the regression the block_family
#    battery caught).
def h_sum(h)
  t = 0
  h.each { |k, v| t = t + k + v }
  t
end
hh = { 1 => 10, 2 => 20, 3 => 30 }
N.times { h_sum(hh) }
p h_sum(hh)

def pair_sum(pairs)
  t = 0
  pairs.each { |a, b| t = t + a * b }
  t
end
pp2 = [[1, 2], [3, 4], [5, 6]]
N.times { pair_sum(pp2) }
p pair_sum(pp2)

def ewi_sum(arr)
  t = 0
  arr.each_with_index { |x, i| t = t + x + i }
  t
end
N.times { ewi_sum(a10) }
p ewi_sum(a10)
