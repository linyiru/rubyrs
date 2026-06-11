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

# Raised by Thread.new-without-block (and CRuby's other
# thread-state errors). Standard hierarchy position.
class ThreadError < StandardError
end

class Thread
  # DEFERRED-EXECUTION green thread (ADR 0017 Rule 4: no OS
  # threads). `Thread.new { ... }` captures the block; it runs — to
  # completion, inline — at the first `join` / `value` call. This
  # gives fork-join shapes (spawn workers, enqueue work, join) the
  # CORRECT RESULTS with zero concurrency:
  #
  #   minitest's Parallel::Executor is the motivating consumer —
  #   its workers loop `while job = queue.pop`; jobs are enqueued
  #   after spawn and the nil terminators before join, so the
  #   drain happens entirely at join time and every test runs.
  #
  # DIVERGENCES (documented, deliberate):
  #   - No preemption/overlap: producer code that BLOCKS waiting
  #     for a worker's progress would deadlock in CRuby terms; the
  #     single-threaded Queue#pop-returns-nil rule (below) is what
  #     keeps the standard pool pattern terminating instead.
  #   - A thread that is never joined never runs.
  #   - Exceptions surface at join (CRuby with
  #     abort_on_exception=false re-raises at join too, so this
  #     edge actually matches).
  def self.new(*args, &block)
    raise ThreadError, "must be called with a block" unless block
    t = allocate
    t.__deferred_init(args, block)
    t
  end

  def __deferred_init(args, block)
    @args = args
    @block = block
    @done = false
    @value = nil
    self
  end

  def __run_deferred
    return if @done
    @done = true
    @value = @block.call(*@args)
  end

  def join(*_limit)
    __run_deferred
    self
  end

  def value
    __run_deferred
    @value
  end

  def alive?
    !@done
  end

  def status
    @done ? false : "run"
  end

  # Worker bodies set `Thread.current.abort_on_exception = true`;
  # Thread.current is the Thread class in this model, so accept the
  # write at class level (and instance level for completeness).
  def self.abort_on_exception=(v)
    v
  end

  def abort_on_exception=(v)
    v
  end
  def self.current
    self
  end
  def self.object_id
    1
  end
  # `Thread.current` IS the Thread class in the single-threaded model,
  # so `Thread.current[:k]` lands here; one process-global store is the
  # correct semantics when there is exactly one thread/fiber.
  #
  # CRuby keeps `#[]`/`#[]=` (FIBER-local) and
  # `#thread_variable_get`/`set` (THREAD-local) in SEPARATE stores, so
  # they must not alias. (rouge's `Formatter.escape_enabled?` reads
  # `Thread.current[:'rouge/with-escape']`, the fiber-local form.)
  def self.[](key)
    @fiber_locals ||= {}
    @fiber_locals[key]
  end
  def self.[]=(key, val)
    @fiber_locals ||= {}
    @fiber_locals[key] = val
  end
  def self.key?(key)
    @fiber_locals ||= {}
    @fiber_locals.key?(key)
  end
  def self.thread_variable_get(key)
    @thread_vars ||= {}
    @thread_vars[key]
  end
  def self.thread_variable_set(key, val)
    @thread_vars ||= {}
    @thread_vars[key] = val
  end
  def self.thread_variable?(key)
    @thread_vars ||= {}
    @thread_vars.key?(key)
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

# Thread::Queue — the single-threaded companion to Mutex /
# ConditionVariable above: a plain FIFO. The semantic divergence is
# `pop` on an empty queue, which CRuby BLOCKS on; with one thread a
# block would deadlock unconditionally, so it returns nil instead
# (the same shape minitest's worker loop uses as its terminator:
# `while job = queue.pop`). `Thread::SizedQueue` is absent.
#
# Motivating consumer: minitest 5.25 (`Minitest::Parallel::Executor`
# builds its job queue at load time; rack's test helper builds a
# warnings queue per spec).
class Thread
  class Queue
    def initialize
      @items = []
      @closed = false
    end
    def push(obj)
      raise ClosedQueueError, "queue closed" if @closed
      @items << obj
      self
    end
    alias << push
    alias enq push
    def pop(_non_block = false)
      @items.shift
    end
    alias deq pop
    alias shift pop
    def empty?
      @items.empty?
    end
    def size
      @items.size
    end
    alias length size
    def num_waiting
      0
    end
    def clear
      @items.clear
      self
    end
    def close
      @closed = true
      self
    end
    def closed?
      @closed
    end
  end
end

# CRuby exposes the same class as top-level ::Queue.
Queue = Thread::Queue

# Raised by push-after-close. CRuby defines it under ::ClosedQueueError
# (subclass of StopIteration).
class ClosedQueueError < StopIteration
end
