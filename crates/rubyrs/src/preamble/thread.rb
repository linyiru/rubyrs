# Thread — rubyrs is single-threaded (ADR 0017 Tier 1 Rule 4: no
# OS threads). Real codebases (tilt, sinatra) use
# `Thread.current.object_id` to derive a unique method-name suffix
# when compiling templates so different compiled bodies don't
# clash. With one thread, a stable integer is enough.
#
# Motivating consumer: tilt-2.7.0 `lib/tilt/template.rb:439`
# computes `method_name = "__tilt_#{Thread.current.object_id.abs}"`
# at every `compile_template_method` call. The integer just has
# to be deterministic and call-time-stable; tilt suffixes the
# template's identity into the rest of the bytecode separately.
#
# Tier 1 shape:
#   - `Thread.current` returns the Thread class itself (a stable
#     singleton-shaped object). Avoids needing a real Thread
#     instance type when no thread semantics exist to model.
#   - `Thread.object_id` returns a constant non-zero integer so
#     `.object_id.abs` (CRuby's tilt pattern) produces the same
#     stable, positive value every call. `Object#object_id` isn't
#     implemented globally — defining it just on the Thread class
#     keeps the contract narrow and the surface obviously a stub.
#
# DIVERGENCE: this is the entire Thread API.
# - `Thread.new {...}` raises NotImplementedError instead of
#   silently allocating an instance and dropping the block (a
#   fail-loud override on top of `Class#new` — otherwise the
#   default allocator would accept the call and quietly ignore
#   the thread body, which is the kind of silent divergence
#   single-threaded shims tend to leak).
# - `Thread.list`, `#join`, `#value`, `Thread.kill`, the whole
#   Mutex/ConditionVariable interplay — all absent.
# - Because `Thread.current` returns the Thread class itself,
#   class-level reflection calls like `.class` / `.name` /
#   `.ancestors` resolve via Class's normal dispatch and DON'T
#   raise NoMethodError. That's a known consequence of the
#   "return self" shape; nothing in tilt's call site exercises
#   them.
# Will revisit if/when Tier 2 ever introduces real concurrency.

class Thread
  def self.new(*args)
    raise NotImplementedError,
      "Thread.new is not supported in single-threaded rubyrs (ADR 0017 Tier 1 Rule 4)"
  end
  def self.current
    self
  end
  def self.object_id
    1
  end
end

# ConditionVariable — single-threaded no-op companion to Mutex.
# Real code pairs `@cond = ConditionVariable.new` with `@cond.wait(
# mutex)` / `signal` / `broadcast` for cross-thread coordination;
# with one thread there is nothing to wait for and no one to signal,
# so every operation degenerates to a no-op returning self.
#
# Motivating consumer: P3 Jekyll spike — jekyll/commands/serve.rb
# builds `@run_cond = ConditionVariable.new` at class-body load time
# (the dev-server run loop, not exercised by `jekyll build`).
#
# DIVERGENCE: `wait` returns immediately instead of blocking. Correct
# for the single-threaded model (a blocking wait with no signaller
# would deadlock); actively wrong if Tier 2 ever adds real threads,
# at which point this shim should be replaced, not extended.
class ConditionVariable
  def initialize
  end
  def wait(*_args)
    self
  end
  def signal
    self
  end
  def broadcast
    self
  end
end
