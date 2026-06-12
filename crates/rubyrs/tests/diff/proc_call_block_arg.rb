# `proc.call(args, &blk)` binds the caller's block into the callee
# proc/lambda's `|.., &b|` slot (Vm::pending_block_arg one-shot →
# Proto::block_param_slot). minitest's Object#stub does
# `val_or_callable.call(*args, &blk)` so a stub lambda can invoke
# the original caller's block.

pr = proc { |x, &b| [x, b ? b.call : :noblk] }
p pr.call(1) { :inner }
p pr.call(2)

l = lambda { |p1, &blk| blk ? blk.call(p1) : :none }
p(l.call(5) { |v| v * 3 })
p l.call(6)

# Block identity survives the forward (mock's expect-block compares
# the received block against the original).
blk2 = proc { "bar" }
got = nil
m = proc { |arg, &b| got = (b == blk2); arg }
m.call("foo", &blk2)
p got

# Square-bracket call form forwards too.
p(pr[3]) # no block -> :noblk arm

# Chained forward: a proc hands its own &b onward.
outer = proc { |&b| b.call(10) }
inner_via = proc { |&b| outer.call(&b) }
p(inner_via.call { |v| v + 7 })

# No cross-invocation leak: a call WITH a block then one WITHOUT.
leak = proc { |&b| b.nil? }
leak.call { :x }
p leak.call
