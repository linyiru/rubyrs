# The Ruby-level Fiber class API (preamble/fiber.rb over the `_fiber`
# host fns): Fiber.new/#resume/Fiber.yield/#alive?, the handle's class,
# and FiberError on a dead resume. Runs under --features _fiber; CRuby
# has Fiber natively.

# run-to-completion (concurrent-ruby's lock_local_var pattern).
f = Fiber.new { 1 + 2 }
p f.resume                       # 3
p f.class                        # Fiber
p f.is_a?(Fiber)                 # true

# yield / resume value round-trip.
g = Fiber.new { a = Fiber.yield(10); b = Fiber.yield(a + 1); "done:#{b}" }
p g.resume                       # 10
p g.resume(100)                  # 101
p g.resume(200)                  # "done:200"

# alive? across the lifecycle.
h = Fiber.new { Fiber.yield }
p h.alive?                       # true (created)
h.resume
p h.alive?                       # true (suspended)
h.resume
p h.alive?                       # false (finished)

# resuming a dead fiber → FiberError (rescuable as StandardError).
begin
  h.resume
rescue FiberError => e
  puts "FiberError: #{e.message}"
end
begin
  h.resume
rescue StandardError
  puts "caught as StandardError"
end

# no block → ArgumentError.
begin
  Fiber.new
rescue ArgumentError => e
  puts "#{e.class}: #{e.message}"
end

# a captured local survives across resume (closure over the body). Uses
# a while loop, not a native iterator — `times`/`each` + Fiber.yield is a
# separate known limitation (the iterator state lives on the Rust stack,
# which Fiber's snapshot can't capture; ADR 0024 territory).
acc = Fiber.new do
  i = 0
  while i < 3
    Fiber.yield(i * i)
    i += 1
  end
  :fin
end
p acc.resume                     # 0
p acc.resume                     # 1
p acc.resume                     # 4
p acc.resume                     # :fin
