# Mutex — rubyrs is single-threaded so the entire lock surface
# degenerates to a no-op. CRuby's actual Mutex enforces real
# exclusion across threads; with one thread the externally
# observable stdout from a non-pathological program is identical
# for both. Real codebases (tilt, sinatra, dry-struct, many gems)
# use Mutex for cache-compilation locks that don't care about the
# lock itself in a single-thread world.
#
# DIVERGENCES not covered (and intentional):
# - `Mutex.new.class` reports `Mutex` here; CRuby says
#   `Thread::Mutex` (Mutex is an alias). No real code branches on
#   this string.
# - Re-entrant `LOCK.synchronize { LOCK.synchronize { ... } }`
#   succeeds here, deadlocks on CRuby. We can't model the real
#   semantics without threads; the divergence is in the
#   user-friendly direction.

LOCK = Mutex.new

# `synchronize { ... }` runs the block, returns the block's value.
puts LOCK.synchronize { 42 }
puts LOCK.synchronize { "hello" }

# Side effects inside the critical section persist outside.
counter = 0
3.times { LOCK.synchronize { counter += 1 } }
puts counter

# Lock state query — fresh mutex is unlocked.
puts LOCK.locked?       # false
# Note: `try_lock` / `lock` / `unlock` actually mutate CRuby's
# real lock state, so calling them in a diff fixture without
# threads makes CRuby deadlock on a follow-up `lock`. We only
# cover them via tilt's actual usage pattern below
# (synchronize-only), which exercises the entire surface that
# real codebases use.

# Cache-compilation pattern — `synchronize { @cache[k] ||= ... }`
# is the load-bearing shape across tilt / sinatra / dry-struct.
# Top-level constants stand in for the @@class-var pattern those
# libs actually use (rubyrs doesn't model `@@`).
CACHE_LOCK = Mutex.new
CACHE = {}
def fetch(k)
  CACHE_LOCK.synchronize do
    CACHE[k] ||= "value-for-#{k}"
  end
end
puts fetch(:a)
puts fetch(:b)
puts fetch(:a)  # cached, same value
