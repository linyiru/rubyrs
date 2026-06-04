# Stack-depth guard parity — runaway recursion through block-
# call paths (`then`, `tap`, `Proc#call`, `yield`) raises
# rescue-able SystemStackError instead of crashing the host.
# Before the dispatch-recursion cap, the rubyrs runtime would
# blow the Rust call stack on these shapes and abort with
# `fatal runtime error: stack overflow`; CRuby raises
# SystemStackError. This fixture locks in the new cap by
# exercising the four most common shapes that trigger the
# bug (each goes through a different Rust path) and asserting
# every one terminates with a catchable SystemStackError.

# Path 1: `Object#then { f }` — block-call via
# `collection_call_block` + `step_block`.
def via_then(x); x.then { |y| via_then(y) }; end
caught = false
begin
  via_then(1)
rescue SystemStackError
  caught = true
end
puts "then=#{caught}"

# Path 2: `Object#tap { f }` — same `collection_call_block`
# path but returns receiver instead of block value; separate
# arm to verify both work.
def via_tap(x); x.tap { via_tap(x) }; end
caught = false
begin
  via_tap(1)
rescue SystemStackError
  caught = true
end
puts "tap=#{caught}"

# Path 3: `Proc#call` recursion via a self-referential lambda
# — exercises the proc-call dispatch path, distinct from
# Object#then's collection_call_block route.
g = nil
g = ->{ g.call }
caught = false
begin
  g.call
rescue SystemStackError
  caught = true
end
puts "lambda=#{caught}"

# Path 4: `yield`-based recursion. Each yield wraps the
# block body in a fresh `dispatch_until`; mutual recursion
# via yield chains them.
def yielder; yield; end
def loop_y; yielder { loop_y }; end
caught = false
begin
  loop_y
rescue SystemStackError
  caught = true
end
puts "yield=#{caught}"

# The trap is rescue-able as `Exception` too (sits under
# Exception, NOT StandardError — same security-posture
# placement as ResourceExhausted and SignalException).
caught_class = nil
begin
  via_then(1)
rescue Exception => e
  caught_class = e.class.to_s
end
puts "as_exception=#{caught_class}"

# Conversely, a bare `rescue` (filters StandardError) MUST
# NOT swallow SystemStackError. If it did, an attacker could
# `rescue => e; retry` an infinite loop into permanent host
# CPU/memory consumption.
bare_caught = nil
outer_caught = nil
begin
  begin
    via_then(1)
  rescue => e
    bare_caught = e.class.to_s
  end
rescue SystemStackError
  outer_caught = "SystemStackError"
end
puts "bare_swallow=#{bare_caught.inspect}"
puts "outer=#{outer_caught}"
