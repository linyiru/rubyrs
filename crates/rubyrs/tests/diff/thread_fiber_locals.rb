# Thread.current[:k] is FIBER-local, thread_variable_* is thread-wide,
# and Fiber.current / Mutex#synchronize stay right inside fibers and
# green threads, where the native serves (#381) step aside.
t = Thread.current

# fiber-locals are per fiber; thread_variable_* is per thread
Thread.current[:fl] = :outer
Thread.current.thread_variable_set(:tv, :outer_tv)
f = Fiber.new do
  a = Thread.current[:fl]
  Thread.current[:fl] = :inner
  [a, Thread.current[:fl], Thread.current.thread_variable_get(:tv)]
end
p f.resume
p Thread.current[:fl]

# Fiber.current inside and outside a fiber
root = Fiber.current
p root.equal?(Fiber.current)
inner = Fiber.new { [Fiber.current.equal?(root), Fiber.current.class] }
p inner.resume
p Fiber.current.equal?(root)

# green thread: its own Thread.current and fiber-locals
th = Thread.new do
  Thread.current[:fl] = :green
  [Thread.current.equal?(t), Thread.current[:fl]]
end
p th.value
p Thread.current[:fl]

m = Mutex.new
f = Fiber.new { m.synchronize { Fiber.yield m.locked?; :out } }
p f.resume, m.locked?, f.resume, m.locked?

# contention from a green thread started inside the block
q = []
m.synchronize do
  w = Thread.new { m.synchronize { q << :waiter } }
  Thread.pass
  q << :holder
  @w = w
end
@w.join
p q
p m.locked?
