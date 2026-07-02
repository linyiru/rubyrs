# Cooperative green-thread scheduling battery (fiber-backed scheduler,
# preamble/thread.rb). Runs under `--features _fiber`; CRuby has real
# preemptive threads — every case here is scheduling-order-DETERMINISTIC
# (fork-join, queue-ordered handoffs, explicit sleeps) so both engines
# print identical bytes.
#
# Known, deliberate divergences NOT asserted here (documented in
# preamble/thread.rb):
#   - main's Thread.current is the Thread class (CRuby: main Thread).
#   - `$!` is process-global across thread switches (CRuby: per-thread).
#     `$~` IS per-thread (FiberSnapshot stashes last_match).
#   - kill lands at the target's next park point (CRuby: async).
#   - a fiber cannot park across a NATIVE-iterator frame (times/each
#     with a blocking call inside — use while; vm/iter.rs truncation).

# --- spawn / join / value ------------------------------------------
t = Thread.new { 21 * 2 }
p t.value
p t.alive?
p t.status

# --- Thread.current identity inside the thread ---------------------
t = Thread.new { Thread.current }
p t.value.equal?(t)

# --- args pass-through ----------------------------------------------
t = Thread.new(3, 4) { |a, b| a * b }
p t.value

# --- queue-ordered producer/consumer handoff ------------------------
q = Queue.new
out = []
producer = Thread.new do
  i = 0
  while i < 5
    q.push(i)
    i += 1
  end
  q.close
end
consumer = Thread.new do
  while (v = q.pop)
    out << v
  end
end
producer.join
consumer.join
p out

# --- two-queue ping-pong (real interleaving, deterministic order) ---
a2b = Queue.new
b2a = Queue.new
log = []
ta = Thread.new do
  i = 0
  while i < 3
    a2b.push("a#{i}")
    log << b2a.pop
    i += 1
  end
  a2b.close
end
tb = Thread.new do
  while (m = a2b.pop)
    log << m
    b2a.push("b-got-#{m}")
  end
end
ta.join
tb.join
p log

# --- Mutex: exclusion + state queries -------------------------------
# Main takes the lock BEFORE spawning the contender, so the order is
# deterministic on preemptive CRuby too.
m = Mutex.new
p m.locked?
order = []
m.lock
p m.locked?
p m.owned?
tm = Thread.new do
  m.synchronize { order << :thread }
end
sleep 0.01 # let the contender park on the held lock
order << :main
m.unlock
tm.join
p order
p m.locked?

# --- ConditionVariable: wait until signalled -------------------------
mtx = Mutex.new
cv = ConditionVariable.new
state = []
waiter = Thread.new do
  mtx.synchronize do
    while state.empty?
      cv.wait(mtx)
    end
    state << :consumed
  end
end
# Let the waiter park first, then signal.
sleep 0.01
mtx.synchronize do
  state << :produced
  cv.signal
end
waiter.join
p state

# --- sleep ordering ---------------------------------------------------
seq = []
ts = Thread.new { sleep 0.05; seq << :slept }
seq << :main_first
ts.join
seq << :after_join
p seq

# --- kill: ensure blocks run, value nil, status false ----------------
hit = []
tk = Thread.new do
  begin
    sleep 5
    hit << :never
  ensure
    hit << :ensure_ran
  end
end
sleep 0.01 # let it park in the sleep
tk.kill
tk.join
p hit
p tk.alive?
p tk.status
p tk.value

# --- kill before the body makes progress: no side effects ------------
# (body opens with a long sleep so preemptive CRuby can't append
# before the kill lands; rubyrs's kill-before-first-run never starts
# the body at all — same observables.)
ran = []
tk2 = Thread.new { sleep 2; ran << :ran }
tk2.kill
tk2.join
p ran
p tk2.alive?

# --- exception: surfaces at join AND at value; status nil ------------
te = Thread.new { raise ArgumentError, "boom" }
begin
  te.join
rescue ArgumentError => e
  puts "join: #{e.message}"
end
begin
  te.value
rescue ArgumentError => e
  puts "value: #{e.message}"
end
p te.status
p te.alive?

# --- join timeout returns nil; thread still alive ---------------------
tj = Thread.new { sleep 5 }
p tj.join(0.02)
p tj.alive?
tj.kill
tj.join
p tj.alive?

# --- thread-locals: fresh store per thread ----------------------------
Thread.current[:battery_key] = :main_value
tl = Thread.new do
  inner_before = Thread.current[:battery_key]
  Thread.current[:battery_key] = :thread_value
  [inner_before, Thread.current[:battery_key]]
end
p tl.value
p Thread.current[:battery_key]

# --- thread_variable_* on instances -----------------------------------
tv = Thread.new { Thread.current.thread_variable_set(:x, 7); Thread.current.thread_variable_get(:x) }
p tv.value

# --- Thread.list includes live threads --------------------------------
gate = Queue.new
lister = Thread.new { gate.pop }
sleep 0.01
p Thread.list.include?(lister)
p Thread.list.size >= 2
gate.push(:go)
lister.join
p Thread.list.include?(lister)

# --- Thread.pass is callable everywhere --------------------------------
tp = Thread.new { Thread.pass; :done }
Thread.pass
p tp.value

# --- $~ is per-thread (FiberSnapshot stashes last_match) ---------------
"main-context" =~ /main-(\w+)/
tr = Thread.new do
  "thread-context" =~ /thread-(\w+)/
  $1
end
p tr.value
p $1

# --- wakeup: a sleeping-forever thread can be woken --------------------
wq = []
tw = Thread.new { sleep; wq << :woke }
sleep 0.01
tw.wakeup
tw.join
p wq

# --- deterministic drain shape (parallel-gem supervisor skeleton) ------
jobs = Queue.new
i = 0
while i < 6
  jobs.push(i)
  i += 1
end
jobs.close
results = []
rmutex = Mutex.new
workers = []
wi = 0
while wi < 3
  workers << Thread.new do
    while (j = jobs.pop)
      rmutex.synchronize { results[j] = j * j }
    end
  end
  wi += 1
end
workers.each(&:join)
p results
