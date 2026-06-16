# Thread — rubyrs has no OS threads (ADR 0017 Tier 1 Rule 4). The
# POSITION is NOT "Thread is unsupported"; it is "the concurrency
# primitives exist but COLLAPSE TO DETERMINISTIC SEQUENTIAL
# EXECUTION." For the patterns real gems actually use — fork-join,
# producer/consumer drained at join, `Mutex.synchronize` around a
# cache — this yields the SAME RESULTS CRuby would, with zero
# concurrency. It DIVERGES (no preemption/overlap, no real
# parallelism, `Queue#pop` on empty returns nil instead of
# blocking) for code that depends on threads overlapping in time.
# Real OS threads are deferred to a future Tier 2 `_thread` gate.
#
# This file is the whole story together with preamble/mutex.rb
# (no-op lock) and the ConditionVariable / Thread::Queue shims
# below. The pieces:
#
#   - `Thread.new { ... }` is a DEFERRED-EXECUTION green thread:
#     it captures the block and runs it inline at the first
#     `join`/`value` (see `self.new` / `__run_deferred` below).
#     Motivating consumer: minitest's Parallel::Executor.
#   - `Thread.current` returns the Thread class itself — a stable
#     singleton-shaped object. With one thread there is no
#     distinct "current thread" to model, and class-level
#     fiber-local / thread-var stores (`[]`, `thread_variable_*`)
#     are process-global, which is the correct semantics for
#     exactly one thread. Consumers: minitest spec.rb
#     (`Thread.current[:current_spec]`), rouge, tilt.
#   - `Thread.object_id` returns a constant non-zero integer so
#     tilt's `"__tilt_#{Thread.current.object_id.abs}"` (tilt
#     2.7.0 template.rb:439) is deterministic and call-time-stable.
#
# DIVERGENCES (documented, deliberate):
#   - `Thread.list`, `Thread.kill`, real scheduling — absent.
#   - Because `Thread.current` returns the Thread class, class-level
#     reflection (`.class` / `.name` / `.ancestors`) resolves via
#     Class's normal dispatch instead of raising; a known, benign
#     consequence of the "return self" shape.
#   - The deferred model is correct ONLY for the drain-at-join
#     shape; a producer that blocks waiting on a worker's progress
#     would deadlock in CRuby and gives wrong results here.

# Raised by Thread.new-without-block (and CRuby's other
# thread-state errors). Standard hierarchy position.
class ThreadError < StandardError
end

class Thread
  # `Thread::Backtrace::Location` — one entry of `caller_locations`.
  # CRuby's are produced by the C backtrace machinery and can't be
  # user-instantiated; rubyrs builds them in the pure-Ruby
  # `Kernel#caller_locations` below by parsing `caller`'s strings, so
  # `path` / `lineno` / `label` track whatever `caller` reports.
  # (`label` is `caller`'s bare method name — rubyrs's `caller` does
  # not yet prefix the defining class the way CRuby 3.4 does, a
  # pre-existing divergence inherited here.)
  class Backtrace
    class Location
      attr_reader :path, :lineno, :label

      def initialize(path, lineno, label)
        @path = path
        @lineno = lineno
        @label = label
      end

      # rubyrs has no separate load-path resolution for backtraces;
      # `caller` already reports the path as written, so absolute and
      # relative coincide here.
      def absolute_path
        @path
      end

      def base_label
        @label
      end

      def to_s
        "#{@path}:#{@lineno}:in '#{@label}'"
      end

      def inspect
        to_s.inspect
      end
    end
  end
end

module Kernel
  # `caller_locations(start = 1, length = nil)` — like `caller` but
  # returns `Thread::Backtrace::Location` objects instead of strings.
  # Implemented over the native `caller`: the `+ 1` skips THIS wrapper
  # frame so `start` is measured from the caller, matching CRuby.
  # zeitwerk's loader uses `caller_locations(1, 1).first.path` to find
  # the file that configured a loader.
  def caller_locations(start = 1, length = nil)
    raw =
      if start.is_a?(Range)
        b = start.begin
        e = start.end
        caller(Range.new(b ? b + 1 : 1, e ? e + 1 : nil, start.exclude_end?))
      elsif length.nil?
        caller(start + 1)
      else
        caller(start + 1, length)
      end
    return nil if raw.nil?
    raw.map do |s|
      m = s.match(/\A(?<path>.*):(?<lineno>\d+):in ['`](?<label>.*)'\z/)
      if m
        Thread::Backtrace::Location.new(m[:path], m[:lineno].to_i, m[:label])
      else
        Thread::Backtrace::Location.new(s, 0, "")
      end
    end
  end
  private :caller_locations
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
    # CRuby: a new thread starts with EMPTY fiber-locals — swap in
    # a fresh store for the block's duration so
    # `Thread.current[:k]` inside the body doesn't see the
    # spawner's values (minitest pins this: `must_equal` inside
    # Thread.new must raise because :current_spec is unset there).
    outer = Thread.instance_variable_get(:@fiber_locals)
    Thread.instance_variable_set(:@fiber_locals, {})
    begin
      @value = @block.call(*@args)
    ensure
      Thread.instance_variable_set(:@fiber_locals, outer)
    end
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

  # `Thread.attr_accessor :x` — in the single-thread model
  # `Thread.current` IS this class, so a class-level `attr_accessor`
  # must expose `Thread.current.x` / `.x=`. Define CLASS-level accessors
  # (backed by a class ivar — process-global, the correct semantics for
  # exactly one thread) AND instance-level ones (for `Thread.new`
  # instances). Avoids `super` to the builtin Module#attr_accessor.
  # Consumer: bridgetown-core/current.rb does
  # `Thread.attr_accessor :bridgetown_state` then
  # `Thread.current.bridgetown_state ||= {}`.
  def self.attr_accessor(*names)
    names.each do |n|
      ivar = :"@#{n}"
      define_singleton_method(n) { instance_variable_get(ivar) }
      define_singleton_method(:"#{n}=") { |v| instance_variable_set(ivar, v) }
      define_method(n) { instance_variable_get(ivar) }
      define_method(:"#{n}=") { |v| instance_variable_set(ivar, v) }
    end
  end

  def self.attr_reader(*names)
    names.each do |n|
      ivar = :"@#{n}"
      define_singleton_method(n) { instance_variable_get(ivar) }
      define_method(n) { instance_variable_get(ivar) }
    end
  end

  def self.attr_writer(*names)
    names.each do |n|
      ivar = :"@#{n}"
      define_singleton_method(:"#{n}=") { |v| instance_variable_set(ivar, v) }
      define_method(:"#{n}=") { |v| instance_variable_set(ivar, v) }
    end
  end

  def self.current
    self
  end
  # Exactly one thread ever exists, and `Thread.current` IS the
  # Thread class, so the live-thread list is the single-element
  # `[self]`. Consumer: rack 2.2.8 reloader.rb guards reloading
  # with `Thread.list.size > 1` (dev-only middleware); size 1
  # gives it the correct single-threaded answer.
  def self.list
    [self]
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
