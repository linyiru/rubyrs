# Ruby-level `Fiber` class API over the `_fiber` host-fn primitives
# (`__rubyrs_fiber_{new,resume,yield,alive_q}`, registered at boot under
# the `_fiber` feature). Loaded only when `_fiber` is built — non-fiber
# builds keep the bare `Fiber` shell from thread.rb (just `.current`).
#
# ADR 0017 places Fiber in Tier 2 ("deferred until a real use case");
# the streaming consumer (ADR 0023) drives the host fns directly, while
# this veneer exposes the standard CRuby `Fiber.new { }.resume` surface
# that ecosystem gems use (concurrent-ruby's lock_local_var probes
# `Fiber.new { mutex.owned? }.resume`).
#
# A fiber handle is a class-less `HeapObj::Fiber` slot; dispatch routes
# its instance methods through this class (see vm/dispatch.rs), so
# `handle.resume` / `handle.alive?` reach the defs below with the handle
# as `self`. A multi-arg resume/yield passes the values as an array; a
# single arg passes the value itself; no args passes nil (CRuby shape).
class Fiber
  def self.new(&block)
    # CRuby's message (it fails creating the underlying Proc).
    raise ArgumentError, "tried to create Proc object without a block" if block.nil?
    __rubyrs_fiber_new(block)
  end

  def resume(*args)
    v = args.length <= 1 ? args[0] : args
    __rubyrs_fiber_resume(self, v)
  end

  def self.yield(*args)
    v = args.length <= 1 ? args[0] : args
    __rubyrs_fiber_yield(v)
  end

  def alive?
    __rubyrs_fiber_alive_q(self)
  end
end
