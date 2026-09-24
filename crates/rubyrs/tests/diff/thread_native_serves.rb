# Thread.current / Thread.current[] / Fiber.current / Mutex#synchronize
# hot paths (#381): the served fast paths must keep the Ruby semantics,
# every block exit, and that overrides win. Fibers: thread_fiber_locals.rb.

# --- Thread.current and its fiber-local store ---
t = Thread.current
p t.equal?(Thread.current)
p Thread.current[:k]
3.times { |i| Thread.current[:k] = i }
p Thread.current[:k]
p Thread.current.key?(:k)
Thread.current[:k] = nil
p Thread.current[:k]

# Fiber.current outside any fiber is one stable root
root = Fiber.current
p root.equal?(Fiber.current)

# --- Mutex#synchronize exits ---
m = Mutex.new
p m.synchronize { 42 }
p m.locked?
p m.synchronize { m.locked? }
p m.synchronize { m.owned? }

def ret_from(m)
  m.synchronize { return :returned }
  :not_reached
end
p ret_from(m)
p m.locked?

r = [1, 2, 3].each { |x| break m.synchronize { break x * 10 } }
p r
p m.locked?

p [1, 2].map { |x| m.synchronize { next x + 1 } }

begin
  m.synchronize { raise ArgumentError, "boom" }
rescue ArgumentError => e
  p e.message
end
p m.locked?

# lock held across a throw
p(catch(:done) { m.synchronize { throw :done, :thrown } })
p m.locked?

# a nested, different mutex
m2 = Mutex.new
p m.synchronize { m2.synchronize { [m.locked?, m2.locked?] } }
p [m.locked?, m2.locked?]


# --- overrides win ---
class SubMutex < Mutex
  def synchronize
    [:sub, super]
  end
end
p SubMutex.new.synchronize { :blk }

sm = Mutex.new
def sm.synchronize = :singleton
p sm.synchronize { :blk }

class Mutex
  alias_method :__orig_synchronize, :synchronize
  def synchronize
    [:reopened, __orig_synchronize { yield }]
  end
end
p Mutex.new.synchronize { :blk }
class Mutex
  alias_method :synchronize, :__orig_synchronize
end
p Mutex.new.synchronize { :restored }

$log = []
class Thread
  class << self
    alias_method :__orig_current, :current
    def current
      $log << :current
      __orig_current
    end
  end
end
Thread.current
p $log
class Thread
  class << self
    alias_method :current, :__orig_current
  end
end
$log = []
Thread.current
p $log
