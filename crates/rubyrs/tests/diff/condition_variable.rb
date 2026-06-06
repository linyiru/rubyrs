# ConditionVariable — single-threaded no-op shim. We assert the
# load-time + signal/broadcast surface (parity-safe); `wait` blocks
# in CRuby so it isn't exercised, and `.class` is namespaced
# differently (Thread::ConditionVariable vs the top-level shim), so
# it isn't asserted. Discovery: P3 Jekyll spike —
# jekyll/commands/serve.rb builds `ConditionVariable.new` at load.
cv = ConditionVariable.new
p cv.is_a?(Object)
p cv.signal.equal?(cv)
p cv.broadcast.equal?(cv)
p cv.respond_to?(:wait)

# Mutex + ConditionVariable pairing constructs without error.
m = Mutex.new
done = m.synchronize { cv.broadcast; 42 }
p done
