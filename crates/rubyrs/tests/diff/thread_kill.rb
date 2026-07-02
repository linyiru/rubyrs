# Thread#kill / #exit / #terminate + Thread.kill — termination in the
# deferred green-thread model. Observable surface pinned here is what
# CRuby's killed/finished threads deterministically report: after
# kill+join `alive?` is false, `status` is false, `value` is nil for a
# killed-before-completion thread and the result for a finished one.
# (Each CRuby-side kill is followed by `join` before inspection —
# CRuby's kill is asynchronous.) Motivating consumer: the parallel
# gem's supervisor (`in_threads` drains `map(&:value)` then `ensure
# threads.each(&:kill)`), which rubocop's default multi-file
# auto-parallel run goes through.
t = Thread.new { sleep 5; puts "never printed" }
t.kill
t.join
p t.alive?
p t.status
p t.value

# kill after completion is a no-op; the computed value is preserved
t2 = Thread.new { 42 }
p t2.value
t2.kill
p t2.value
p t2.alive?

# exit / terminate aliases
t3 = Thread.new { sleep 5 }
t3.exit
t3.join
p t3.alive?
t4 = Thread.new { sleep 5 }
t4.terminate
t4.join
p t4.alive?

# kill returns the thread itself, so does the class-level form
t5 = Thread.new { sleep 5 }
p Thread.kill(t5).equal?(t5)
t5.join
p t5.alive?
t6 = Thread.new { sleep 5 }
p t6.kill.equal?(t6)
t6.join

# Thread.kill type-checks its argument
begin
  Thread.kill(42)
rescue TypeError => e
  puts e.message
end

# The parallel-gem supervisor shape: value-drain, then kill-all
threads = 3.times.map { |i| Thread.new { i * 10 } }
p threads.map(&:value)
threads.each(&:kill)
threads.each(&:join)
p threads.map(&:alive?)
