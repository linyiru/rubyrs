# A share-direct block frame runs in its creator's cell, so a second live
# invocation of the same block must take the copy path. Fibers park live
# frames outside the running stack (a suspended fiber's snapshot, or the
# resumer's stack while a fiber runs); each shape below re-enters a block
# whose first invocation is parked that way.

# Suspended inside a fiber, re-invoked from main (nested share: the proc
# is created in the fiber body block).
pr = nil
f = Fiber.new do
  pr = proc { |v| t = v; Fiber.yield(:susp) if v == 1; t }
  pr.call(1)
end
p f.resume
p pr.call(2)
p f.resume

# The same at method scope (plain share-direct).
def m
  pr = proc { |v| t = v; Fiber.yield(:susp) if v == 1; t }
  f = Fiber.new { pr.call(1) }
  a = f.resume
  b = pr.call(2)
  [a, b, f.resume]
end
p m

# Two fibers suspended in the same nested block.
l = -> do
  q = proc { |v| t = v * 10; Fiber.yield(t); t + 1 }
  f1 = Fiber.new { q.call(1) }
  f2 = Fiber.new { q.call(2) }
  [f1.resume, f2.resume, f1.resume, f2.resume]
end
p l.call

# The resumer's invocation is parked while the fiber re-enters the block.
g = nil
r = proc { |v| t = v; g.resume if v == 1; t }
g = Fiber.new { r.call(2) }
p r.call(1)
l2 = -> do
  h = nil
  q = proc { |v| t = v; h.resume if v == 1; t }
  h = Fiber.new { q.call(2) }
  q.call(1)
end
p l2.call

# A fiber abandoned while suspended, then collected: blocks still bind
# correctly afterwards.
3.times { Fiber.new { [1].each { |x| y = x } ; Fiber.yield 1 }.resume }
GC.start
p (-> { s = 0; [1, 2, 3].each { |i| [i].each { |j| s += j } }; s }).call
