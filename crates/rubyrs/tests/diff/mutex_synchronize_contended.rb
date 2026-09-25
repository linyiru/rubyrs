# Mutex#synchronize with a waiter parked on the lock: the native
# serve (vm/thread.rs) must hand the unlock to the Ruby path whenever
# `@waiters` is non-empty, so the parked green thread is woken on
# every way out of the block (value, exception, next, return).
m = Mutex.new
log = []
t = nil
m.synchronize do
  t = Thread.new { m.synchronize { log << :t } }
  sleep 0.01
  log << :main
end
t.join
p log, m.locked?

log = []
t2 = nil
begin
  m.synchronize do
    t2 = Thread.new { m.synchronize { log << :t2 } }
    sleep 0.01
    raise "boom"
  end
rescue => e
  log << e.message
end
t2.join
p log, m.locked?

def brk(m, log)
  t3 = nil
  r = m.synchronize do
    t3 = Thread.new { m.synchronize { log << :t3 } }
    sleep 0.01
    next 5 if log.empty?
    6
  end
  t3.join
  [r, log]
end
p brk(m, [])

def ret(m, log)
  $t4 = nil
  m.synchronize do
    $t4 = Thread.new { m.synchronize { log << :t4 } }
    sleep 0.01
    return :r
  end
end
lg = []
p ret(m, lg)
$t4.join
p lg, m.locked?

q = Queue.new
t5 = Thread.new { m.synchronize { q << :in; sleep 0.01; :t5 } }
q.pop
m.synchronize { p :main_got_it }
p t5.value, m.locked?
m.lock
p m.locked?, m.owned?
m.unlock
p m.locked?
