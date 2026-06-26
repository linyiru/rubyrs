# A `define_singleton_method` whose body `yield`s captures the ENCLOSING
# method's block (here on_teardown's block) via the closure's
# captured_yield_block. Once the defining scope returns, that block is
# reachable ONLY through the singleton method's closure, which lives on the
# INSTANCE's eigenclass (not in the Vm class table). GC must trace the
# eigenclass method-closure's captured_yield_block, or the block is swept and
# the later `teardown` yields into a dead slot (use-after-free). This is
# minitest's `on_teardown` idiom; the churn below crosses the GC threshold
# several times between capture and the eventual yield.
module OnTeardown
  def on_teardown
    define_singleton_method(:teardown) do
      yield
      super()
    end
  end
end

class Base
  def teardown
    puts "base teardown"
  end
end

class T < Base
  include OnTeardown
end

def run_one(t)
  t.on_teardown { puts "torn down" }       # block captured by the closure
  # Heavy allocation churn AFTER the defining scope returned: forces GC to
  # run while the captured block is reachable only via the eigenclass closure.
  acc = 0
  200_000.times { |i| acc += [i, "s#{i}", { k: i }].length }
  puts acc
  t.teardown                                # yields to the captured block
end

run_one(T.new)
