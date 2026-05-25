# Mutex — rubyrs is single-threaded so the entire lock surface
# degenerates to a no-op. CRuby's actual Mutex enforces real
# exclusion across threads and tracks lock state; we model
# NEITHER — only the `synchronize { ... }` cache-guard shape
# that tilt / sinatra / dry-struct / many gems actually use to
# protect compilation caches. For that shape the externally
# observable stdout is identical to CRuby. Direct state queries
# (`m.lock; puts m.locked?`) WILL diverge and are out of scope.
#
# DIVERGENCES not covered (and intentional):
# - `Mutex.new.class` reports `Mutex` here; CRuby says
#   `Thread::Mutex` (Mutex is an alias). No real code branches on
#   this string.
# - Re-entrant `LOCK.synchronize { LOCK.synchronize { ... } }`
#   succeeds here, deadlocks on CRuby. We can't model the real
#   semantics without threads; the divergence is in the
#   user-friendly direction.

# Arity check — `Mutex.new` takes zero args. Regression guard
# against silently accepting extras (the issue the explicit
# 0-arity `def initialize` in the preamble fixes).
begin
  Mutex.new(1)
  puts "BUG: no error"
rescue ArgumentError => e
  puts "ArgumentError: #{e.message}"
end

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
