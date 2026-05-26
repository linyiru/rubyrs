# Mutex — rubyrs is single-threaded, so the entire lock surface
# degenerates to "run the block / no-op". Real codebases use
# `LOCK = Mutex.new` + `LOCK.synchronize { ... }` to wrap
# compilation caches (tilt, sinatra, dry-struct all do this);
# with one thread the critical section is already exclusive.
# `try_lock` returns true (the lock is always available);
# `locked?` returns false (we never actually hold one).
# Re-entrant `synchronize` "just works" because there's no
# real lock state to deadlock against.
#
# This file ships per ADR 0017 Tier 1 Rule 4 (no OS threads); if
# Tier 2 ever adds real concurrency, this shim should be replaced
# rather than extended — pretending to lock is fine when there's
# nothing to lock against, but actively wrong once parallelism
# enters the picture.

class Mutex
  # CRuby's Mutex.new takes zero args; defining an explicit
  # 0-arity initialize delegates arity-checking to the existing
  # method-call machinery so `Mutex.new(1)` raises ArgumentError
  # instead of silently dropping the arg.
  def initialize
  end
  # `synchronize` requires a block — CRuby raises ThreadError on
  # bare call; we raise RuntimeError ("no block given (yield)")
  # via the bare yield. Different exception class, same fail-loud
  # semantics; no realistic code depends on the class name here.
  def synchronize
    yield
  end
  def lock
    self
  end
  def unlock
    self
  end
  def try_lock
    true
  end
  def locked?
    false
  end
  def owned?
    false
  end
end
