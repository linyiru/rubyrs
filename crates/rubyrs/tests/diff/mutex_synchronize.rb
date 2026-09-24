# Mutex#synchronize / #lock / #unlock, served natively (vm/thread.rs)
# in the uncontended case: the lock is released on every way out of
# the block (value, next, break, return, throw, exception), re-entry
# nests, and a user override of synchronize still wins. (CRuby's C
# synchronize never calls a Ruby-level lock/unlock override; rubyrs's
# does, so that shape is pinned by the unit tests in vm/thread.rs.)
m = Mutex.new
p m.synchronize { 5 }, m.locked?
p m.synchronize { next 6 }, m.locked?
p [1, 2].each { break m.synchronize { break 42 } }, m.locked?
def ret(m) = m.synchronize { return 7 }
p ret(m), m.locked?
p catch(:t) { m.synchronize { throw :t, 9 } }, m.locked?
begin
  m.synchronize { raise "boom" }
rescue => e
  p [e.message, m.locked?]
end
begin
  m.synchronize { [1].each { raise ArgumentError, "deep" } }
rescue ArgumentError => e
  p [e.message, m.locked?]
end
p m.synchronize { [m.locked?, m.owned?] }, m.locked?
x = 0
i = 0
while i < 1000
  m.synchronize { x += 1 }
  i += 1
end
p x, m.locked?
p m.lock.equal?(m), m.locked?, m.owned?
p m.unlock.equal?(m), m.locked?
p m.try_lock, m.try_lock, m.locked?
m.unlock
p m.locked?
p Mutex.new.synchronize { |*a| a }
p((Mutex.new.synchronize rescue $!.class))
p Thread::Mutex.equal?(Mutex)

class SubMutex < Mutex
  def extra = :extra
end
sm = SubMutex.new
p sm.synchronize { [sm.locked?, sm.extra] }, sm.locked?
def m.tag = :tagged
p m.synchronize { m.tag }, m.locked?

class Mutex
  alias_method :orig_synchronize, :synchronize
  def synchronize(&b)
    puts "patched"
    orig_synchronize(&b)
  end
end
p Mutex.new.synchronize { :patched_ok }
