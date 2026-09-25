# `Thread.current[:k]` is FIBER-local (each Fiber.new body starts with
# an empty store, and its writes stay in it), while
# `thread_variable_get/set` is THREAD-local (shared by every fiber of
# the thread). The key contract itself is pinned by thread_local_keys.
t = Thread.current
t[:a] = 1
t.thread_variable_set(:tv, 9)
f = Fiber.new do
  p [:in, Thread.current[:a], Thread.current.key?(:a), Thread.current.keys]
  p [:tv, Thread.current.thread_variable_get(:tv), Thread.current.equal?(t)]
  Thread.current[:a] = 2
  Thread.current[:b] = 3
  Thread.current.thread_variable_set(:tv2, 10)
  p [:fiber_current, Fiber.current.equal?(f), Fiber.current.equal?(Fiber.current)]
  Fiber.yield
  p [:in2, Thread.current[:a], Thread.current[:b]]
end
f.resume
p [:out, t[:a], t[:b], t.key?(:b), t.thread_variable_get(:tv2)]
f.resume
p [:out2, t[:a], t.keys.sort]
Fiber.new { Fiber.new { Thread.current[:a] = :deep; p Thread.current[:a] }.resume; p Thread.current[:a] }.resume
p t[:a]
g = Fiber.new { Thread.current[:a] = :g; Fiber.yield; Thread.current[:a] }
g.resume
p [t[:a], g.resume]
p Fiber.current.equal?(Fiber.current)

# Fiber-locals inside a green thread belong to that thread; the main
# thread's store stays reachable through `Thread.main`.
th = Thread.new do
  Thread.current[:a] = :thread
  [Thread.current[:a], Thread.current.key?(:x), Thread.main[:a]]
end
p th.value, t[:a]
th[:k] = 1
th["k2"] = 2
th[:k2] = nil
p [th[:k], th["k"], th.key?(:k), th.keys.sort]
p((th[1] rescue $!.class))

# --- from #395: the serves stepping aside inside fibers / green threads.
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
