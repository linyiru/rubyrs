# LITE REST-BLOCK battery (ADR 0037 tail): the frameless `|*a|` entry —
# the serve site allocates the rest Array BEFORE entering native state and
# the entry binds it like a plain param — must be observably identical to
# the framed binder. Loops run past the tier-2 compile threshold so the
# lite entries actually serve (under RUBYRS_JIT_TIER2 configs; plain
# configs cover the interpreter binder byte-for-byte).
N = 300

# 1. Splat identity: the callee's rest Array is FRESH per call — an
#    in-block mutation must not leak into the next invocation (or into
#    the yielded element).
def splat_fresh(arr)
  lens = []
  arr.each { |*a| a << :x; lens << a.length }
  lens
end
a10 = (1..10).to_a
N.times { splat_fresh(a10) }
p splat_fresh(a10).uniq
p a10

# 2. A lone Array arg stays INTACT for a rest-only block (no auto-splat).
def splat_intact(pairs)
  out = []
  pairs.each { |*a| out << a }
  out
end
pairs = [[1, 2], [3, 4]]
N.times { splat_intact(pairs) }
p splat_intact(pairs)

# 3. rest + next-with-value (a lite block's Op::Return).
def rest_next(arr)
  arr.map { |*a| next a[0] * 10 if a[0].odd?; a[0] }
end
N.times { rest_next(a10) }
p rest_next(a10)

# 4. rest + break through the driver (break declines admission — the
#    body stays interpreted end-to-end; the SHAPE must still bind right).
def rest_break(arr)
  arr.each { |*a| break a[0] + 100 if a[0] == 7 }
end
N.times { rest_break(a10) }
p rest_break(a10)

# 5. rest-block capture-write: writes land in the defining scope's cells,
#    visible to a sibling block and after the loop.
def rest_cap(arr)
  t = 0
  arr.each { |*a| t = t + a[0] }
  arr.each { |*a| t = t + a.length }
  t
end
N.times { rest_cap(a10) }
p rest_cap(a10)

# 6. rest-only LAMBDA (arity 0+ always in range — serves like a proc).
f = ->(*a) { a.length * 100 + a.fetch(0, 0) }
t = 0
N.times { a10.each { |x| t = f.call(x) } }
p t
p f.call
p f.call(1, 2)

# 7. yield shapes into |*a|: 0-arg (rest == []), 2-arg (rest == [x, y]) —
#    both outside the 1-arg serve site, exercising the general binder
#    against the same proto.
def yielder0 = yield
def yielder2 = yield 5, 6
def rest_all_shapes
  r = []
  yielder0 { |*a| r << a }
  yielder2 { |*a| r << a }
  [1].each { |*a| r << a }
  r
end
N.times { rest_all_shapes }
p rest_all_shapes

# 8. STRESS_GC acid shape: the rest Array is allocated at the serve site
#    and must stay rooted across the whole frameless body — heap values
#    inside it must survive a collection mid-loop.
def rest_gc(strs)
  out = []
  strs.each { |*a| out << a[0].length }
  out.sum
end
strs = %w[alpha beta gamma delta epsilon]
N.times { rest_gc(strs.map(&:dup)) }
p rest_gc(strs)

# 9. Re-entrant rest-block recursion (copy-path / reentrancy handling).
def rec_walk(n, &blk)
  return if n.zero?
  [n].each { |*a| blk.call(a[0]); rec_walk(a[0] - 1, &blk) }
end
acc = []
N.times { acc = []; rec_walk(5) { |v| acc << v } }
p acc
