# Scheduler yield points inside NATIVE (Rust-driven) iterators must
# not truncate the iteration. Pre-fix, a green thread running
# `[0,1,2].each { |i| p i; Thread.pass }` printed only `0`: the park
# point's `Fiber.yield` could not stash the Rust-level iterator loop,
# and vm/iter.rs step_block's fiber_yield_pending guard silently
# dropped the remaining elements (`map` even returned the last block
# value instead of an array). Fixed by probing
# `__rubyrs_fiber_can_yield` at every coop park point
# (preamble/thread.rb __coop_yield_parked): when a native frame pins
# the fiber, the thread drives the scheduler INLINE (main-style)
# instead of yielding — semantically exact, no truncation.
#
# Every assertion here is scheduling-independent (per-thread
# sequences, totals, return values) so preemptive CRuby prints
# identical bytes. Nested inline parks stack re-entrant dispatch
# levels; the debug-profile dispatch cap (vm/gc.rs
# DEFAULT_MAX_DISPATCH_DEPTH) was raised 5 → 8 to accommodate the
# deepest shape here (`each { sleep 0 }` in a green thread).

# --- the reported shape: each + Thread.pass in a green thread -------
out = []
t = Thread.new { [0, 1, 2].each { |i| out << i; Thread.pass } }
t.join
p out

# --- the whole native-iterator family, one green thread -------------
t = Thread.new do
  r = {}
  r[:map] = [1, 2, 3].map { |i| Thread.pass; i * 10 }
  r[:times] = []
  3.times { |i| r[:times] << i; Thread.pass }
  r[:select] = [1, 2, 3, 4].select { |i| Thread.pass; i.odd? }
  r[:ewi] = []
  %w[a b].each_with_index { |s, i| r[:ewi] << [s, i]; Thread.pass }
  r[:sum] = [1, 2, 3].sum { |i| Thread.pass; i }
  r
end
r = t.value
p r[:map]
p r[:times]
p r[:select]
p r[:ewi]
p r[:sum]

# --- nested native iterators around the yield point ------------------
pairs = []
t = Thread.new do
  [0, 1].each do |i|
    [0, 1].each do |j|
      pairs << [i, j]
      Thread.pass
    end
  end
end
t.join
p pairs

# --- sleep(0) as the yield shape inside a native iterator ------------
slept = []
t = Thread.new { [7, 8, 9].each { |i| slept << i; sleep 0 } }
t.join
p slept

# --- producer/consumer: consumer pops INSIDE a native iterator -------
# (producer parks in pure-Ruby frames; consumer's Queue#pop park sits
# under Integer#times and must survive via the inline drive)
q = Queue.new
got = []
cons = Thread.new { 3.times { got << q.pop } }
prod = Thread.new do
  vals = [10, 20, 30]
  i = 0
  while i < vals.length
    q << vals[i]
    i += 1
    Thread.pass
  end
end
cons.join
prod.join
p got

# --- interleaving: native-pinned thread ping-pongs a pure-Ruby one ---
# (global interleave order is scheduler-specific; assert the
# per-thread projections, which any correct scheduler preserves)
log = []
ta = Thread.new { [1, 2, 3].each { |i| log << "a#{i}"; Thread.pass } }
tb = Thread.new do
  i = 1
  while i <= 3
    log << "b#{i}"
    i += 1
    Thread.pass
  end
end
ta.join
tb.join
p log.select { |e| e.start_with?("a") }
p log.select { |e| e.start_with?("b") }
p log.length

# --- BOTH threads natively pinned at their parks ----------------------
# (nested inline drives: the second thread's park rotates past the
# stack-pinned first thread instead of double-resuming its fiber)
log2 = []
tc = Thread.new { [1, 2, 3].each { |i| log2 << "c#{i}"; Thread.pass } }
td = Thread.new { [1, 2, 3].each { |i| log2 << "d#{i}"; Thread.pass } }
tc.join
td.join
p log2.select { |e| e.start_with?("c") }
p log2.select { |e| e.start_with?("d") }
p log2.length

# --- Mutex#synchronize inside a native iterator ----------------------
m = Mutex.new
order = []
tm = Thread.new { [1, 2].each { |i| m.synchronize { order << "m#{i}" }; Thread.pass } }
tn = Thread.new do
  i = 1
  while i <= 2
    m.synchronize { order << "n#{i}" }
    i += 1
    Thread.pass
  end
end
tm.join
tn.join
p order.select { |e| e.start_with?("m") }
p order.select { |e| e.start_with?("n") }

# --- thread completes; value/alive?/status after the pinned parks ----
t = Thread.new { [1, 2, 3].map { |i| Thread.pass; i * 2 } }
p t.value
p t.alive?
p t.status
