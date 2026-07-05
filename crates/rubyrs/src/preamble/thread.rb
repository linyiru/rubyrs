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
#   - Real scheduling — absent. `Thread#kill` (below) marks the
#     deferred thread dead; a thread killed BEFORE its first
#     `join`/`value` never runs AT ALL, whereas CRuby's would already
#     have been scheduled and may have partially executed. Correct
#     for the supervisor shape (kill = "don't want this work"), wrong
#     for code that relies on pre-kill side effects.
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
      # `Regexp.new` (not a `/…/` literal) so this preamble parses in a
      # regex-off build (ADR 0017 Rule 3); backtrace parsing then degrades
      # to a runtime error there instead of an ICE at preamble load.
      m = s.match(Regexp.new("\\A(?<path>.*):(?<lineno>\\d+):in ['`](?<label>.*)'\\z"))
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
    if __coop_supported?
      t.__coop_init(args, block)
    else
      t.__deferred_init(args, block)
    end
    t
  end

  # `Thread.current.status` — since `Thread.current` is the Thread class
  # (single-threaded model), this class method answers for the running
  # thread: always `"run"`, never `"aborting"`. ActiveRecord's
  # `within_new_transaction` branches on `Thread.current.status == "aborting"`.
  def self.status
    "run"
  end

  def __deferred_init(args, block)
    @args = args
    @block = block
    @done = false
    @value = nil
    self
  end

  # A deferred green thread's status: finished threads report `false`
  # (CRuby's terminated-thread value), otherwise `"run"`.
  # Cooperative threads additionally report CRuby's full surface:
  # `nil` for terminated-by-exception, `"sleep"` while parked.
  def status
    if @coop
      return @exception ? nil : false if @done
      return @park_ref || @sleep_ref ? "sleep" : "run"
    end
    @done ? false : "run"
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

  def join(*limit)
    return __coop_join(limit[0]) if @coop
    __run_deferred
    self
  end

  def value
    if @coop
      __coop_join(nil)
      return @value
    end
    __run_deferred
    @value
  end

  def alive?
    !@done
  end

  # `Thread#kill` / `#exit` / `#terminate` — terminate the thread.
  # In the deferred model "terminate" is marking it done WITHOUT
  # running the captured block: a killed-before-first-join thread
  # never executes (see the DIVERGENCES note in the header), and a
  # kill after completion is a no-op — both report `alive?` false /
  # `status` false and `value` nil-or-result afterwards, matching
  # what CRuby's killed/finished threads observably report. Returns
  # self (CRuby returns the thread). Motivating consumer: the
  # parallel gem's supervisor (`in_threads` runs
  # `threads.map(&:value)` then `ensure threads.each(&:kill)`, and
  # `work_in_processes` kills workers via `w.thread&.kill` on the
  # exception path) — rubocop's default multi-file run auto-enables
  # --parallel through it.
  def kill
    return __coop_kill if @coop
    @done = true
    self
  end
  alias exit kill
  alias terminate kill

  # `Thread.kill(thread)` — class-level form, same termination.
  # CRuby type-checks the argument (TypeError, not NoMethodError,
  # for a non-Thread).
  def self.kill(thread)
    unless thread.is_a?(Thread)
      raise TypeError, "wrong argument type #{thread.class} (expected VM/thread)"
    end
    thread.kill
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

  # `Thread.current` — on the main thread this is the Thread class
  # itself (the documented single-threaded shape every existing
  # consumer relies on: tilt's object_id suffix, minitest's
  # fiber-locals, rouge). INSIDE a cooperative green thread (the
  # fiber-backed scheduler below) it is that thread's Thread
  # instance — the parallel gem's supervisor does
  # `worker.thread = Thread.current` and later `w.thread&.kill`,
  # which needs real per-thread identity. `@coop_current` is set by
  # the scheduler strictly around each resume, so the check is one
  # ivar read on the main-thread fast path.
  def self.current
    @coop_current || self
  end
  # The live-thread list: the main-thread sentinel (`self` — the
  # documented divergence that main IS the Thread class) plus every
  # live cooperative green thread. Consumer: rack 2.2.8 reloader.rb
  # guards reloading with `Thread.list.size > 1` (dev-only
  # middleware); with no green threads this stays the single-element
  # `[self]` it always was.
  def self.list
    live = @coop_threads ? @coop_threads.select { |t| t.alive? } : []
    [self] + live
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

  # `Thread.handle_interrupt(ExceptionClass => :never|:immediate|:on_blocking)
  # { ... }` masks asynchronous interrupts around the block. With one
  # thread there are no async interrupts to defer, so it degenerates to
  # running the block and returning its value. connection_pool's `with`
  # wraps checkout/checkin in `Thread.handle_interrupt(Exception =>
  # :never) { ... }`.
  def self.handle_interrupt(_mask)
    yield
  end

  # Companion predicate — no interrupts are ever pending in the
  # single-threaded model.
  def self.pending_interrupt?(*_args)
    false
  end
end

# Fiber — Tier 1 models ONLY the Ruby 3.2+ "fiber storage" API
# (`Fiber[]` / `Fiber[]=`), NOT the control-flow primitive
# (`Fiber.new` / `#resume` / `Fiber.yield`). In the single-fiber
# model there is exactly one storage scope, so a process-global hash
# is the correct backing (CRuby's fiber storage is inheritable to
# child fibers, which never exist here). Kept SEPARATE from Thread's
# fiber-local store (`Thread.current[:k]`) — CRuby keeps the two
# distinct. multi_json caches its per-call adapter override in
# `Fiber[:multi_json_adapter]`.
#
# CRuby contract: keys must be a Symbol or a String (else TypeError
# "<key.inspect> is not a symbol nor a string"); a missing key reads
# as nil; the setter returns the assigned value (assignment semantics).
class Fiber
  # `Fiber.current` — the running fiber. Under the `_fiber` feature a
  # real fiber body returns the actual fiber (via the host fn); at the
  # top level (and in non-fiber builds) there's one implicit root fiber,
  # so return a stable per-process sentinel. Consumers use it as a Hash
  # key for per-fiber state (logger's `level_key` keys the log level on
  # `Fiber.current`); a stable object is all that's required.
  def self.current
    cur = (__rubyrs_fiber_current rescue nil)
    return cur unless cur.nil?
    @root_fiber ||= Object.new
  end

  def self.[](key)
    unless key.is_a?(Symbol) || key.is_a?(String)
      raise TypeError, "#{key.inspect} is not a symbol nor a string"
    end
    @storage ||= {}
    @storage[key]
  end
  def self.[]=(key, val)
    unless key.is_a?(Symbol) || key.is_a?(String)
      raise TypeError, "#{key.inspect} is not a symbol nor a string"
    end
    @storage ||= {}
    @storage[key] = val
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
    @waiters = []
  end
  # With the cooperative scheduler live, `wait` releases the mutex,
  # parks until signal/broadcast (or the timeout), and re-acquires.
  # Single-threaded world keeps the historical no-op (returning
  # immediately — a real wait with no signaller would deadlock).
  def wait(mutex = nil, timeout = nil)
    if ::Thread.__coop_active?
      deadline = timeout ? ::Thread.__coop_now + timeout : nil
      mutex.unlock if mutex
      begin
        ::Thread.__coop_block(@waiters, deadline)
      ensure
        mutex.lock if mutex
      end
    end
    self
  end
  def signal
    ::Thread.__coop_wake_one(@waiters) unless @waiters.empty?
    self
  end
  def broadcast
    ::Thread.__coop_wake_all(@waiters) unless @waiters.empty?
    self
  end
  # MonitorMixin::ConditionVariable convenience loops. ActiveRecord's
  # connection pool waits on resource availability via these. In the
  # single-threaded no-op model `wait` makes no progress, so the loop is a
  # plain predicate check — which is exactly the common path (the resource
  # is already available, so the block is false/true on the first test).
  def wait_while
    while yield
      wait
    end
  end
  def wait_until
    until yield
      wait
    end
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
      @waiters = []
    end
    def push(obj)
      raise ClosedQueueError, "queue closed" if @closed
      @items << obj
      ::Thread.__coop_wake_one(@waiters) unless @waiters.empty?
      self
    end
    alias << push
    alias enq push
    # `pop` on empty: with the cooperative scheduler live this BLOCKS
    # (parks) until a push/close — CRuby semantics. In the
    # single-threaded world it keeps the documented nil-return shape
    # (a block would deadlock unconditionally; minitest's worker loop
    # uses the nil as its terminator).
    def pop(non_block = false)
      if @items.empty? && !@closed && !non_block && ::Thread.__coop_active?
        while @items.empty? && !@closed
          ::Thread.__coop_block(@waiters, nil)
        end
      end
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
      @waiters.size
    end
    def clear
      @items.clear
      self
    end
    def close
      @closed = true
      ::Thread.__coop_wake_all(@waiters) unless @waiters.empty?
      self
    end
    def closed?
      @closed
    end
  end
end

# CRuby exposes the same class as top-level ::Queue.
Queue = Thread::Queue

# CRuby defines Mutex and ConditionVariable both at the top level and
# nested under Thread (`Thread::Mutex.equal?(Mutex)` is true), the
# nested name being the canonical one. rubyrs defines them top-level
# (mutex.rb / above); alias the nested constants so the
# `Thread::Mutex.new` / `Thread::ConditionVariable.new` form resolves —
# connection_pool's TimedStack uses it.
class Thread
  Mutex = ::Mutex
  ConditionVariable = ::ConditionVariable
end

# Raised by push-after-close. CRuby defines it under ::ClosedQueueError
# (subclass of StopIteration).
class ClosedQueueError < StopIteration
end

# ---------------------------------------------------------------------
# Cooperative green-thread scheduler (fiber-backed).
#
# When the `_fiber` build is present (preamble/fiber.rb defines
# `Fiber::RUBYRS_COOP`), `Thread.new` creates a REAL runnable green
# thread instead of the deferred stub above: the body runs in a Fiber,
# and BLOCKING OPERATIONS BECOME YIELD POINTS — a thread that would
# block on a pipe read/write, `join`, `Mutex#lock`, `Queue#pop`,
# `ConditionVariable#wait` or `sleep` parks itself and yields back to
# the scheduler, which resumes the next runnable thread. The MAIN
# thread is the scheduler: it is not a fiber; when main blocks (join,
# fd read, sleep) it drives `__coop_schedule_step` inline until its own
# wake condition holds. When nothing is runnable the scheduler blocks
# in ONE poll(2) over every fd-parked thread (+ a timeout for timed
# parks), waking the ready ones.
#
# Motivating consumer: the parallel gem's `work_in_processes`
# supervisor (rubocop --parallel). Its N supervisor threads each loop
# { pop job index under a Mutex; Marshal frame to the worker's pipe;
# BLOCKING pipe read of the result }. Under the deferred model the
# first supervisor fed its worker every job sequentially (~1.0x);
# with fd parks the N pipes are serviced concurrently and the forked
# workers actually run in parallel.
#
# Design constraints honored:
#   - ZERO overhead when no green thread is live: every hook
#     (`Thread.current`, fd reads, Mutex, Queue, sleep) checks a
#     single class-ivar flag (`@coop_live > 0` / `@coop_current`)
#     before touching scheduler machinery.
#   - Fibers can suspend arbitrary PURE-RUBY frame chains, but NOT
#     across a native (Rust-driven) iterator frame — a `Fiber.yield`
#     under `Array#each`/`Integer#times`/... truncates the iteration
#     (see vm/iter.rs step_block). Park points therefore probe
#     `__rubyrs_fiber_can_yield` and, when a native frame pins the
#     fiber, fall back to MAIN-style INLINE scheduling
#     (`__coop_wait_inline`) instead of yielding — no truncation.
#     Every park point in the supervisor flow (Marshal frame
#     read/write loops, mutex, join) is still reached through
#     pure-Ruby frames only (`loop` is pure Ruby, Op::Yield block
#     calls suspend correctly), so the fast fiber-switch path stays
#     the norm.
#   - fork(2): the preamble fork wrapper calls
#     `Thread.__coop_after_fork!` in the child — only the forking
#     thread survives (POSIX), so the child resets to a single-thread
#     world and parent-parked fds are never double-polled.
#
# Divergences (documented):
#   - Main thread's `Thread.current` remains the Thread class (the
#     long-standing single-thread shape all existing consumers pin).
#   - Scheduling is cooperative: a green thread that never blocks runs
#     to completion once resumed; `Thread#kill` on it takes effect at
#     its next park point (CRuby's kill is asynchronous).
#   - `$!`/`$?` are process-global, not per-thread; a thread switch
#     inside a rescue does not save/restore them. The parallel-gem
#     flow binds exceptions to locals (`rescue => e`) and never reads
#     `$?` parent-side, so nothing observes this today.
class Thread
  @coop_live = 0
  @coop_current = nil
  @coop_threads = []
  @coop_runq = []
  @coop_fd_r = {}
  @coop_fd_w = {}
  @coop_sleepers = []

  # Raised inside a green thread's fiber at its park point when the
  # thread is killed. Subclasses Exception (not StandardError) so the
  # supervisor pattern `rescue StandardError` doesn't swallow a kill;
  # ensure blocks along the unwind DO run (CRuby kill semantics).
  # Internal — user code should never rescue it by name.
  class CoopKill < Exception
  end

  class << self
    # Feature probe, memoized once: real scheduling needs the fiber
    # machinery (`_fiber` build). `RUBYRS_COOP_THREADS=0` forces the
    # deferred model on a fiber build (A/B measurement hatch).
    def __coop_supported?
      s = @coop_supported
      return s unless s.nil?
      @coop_supported =
        !!defined?(::Fiber::RUBYRS_COOP) &&
        (!defined?(ENV) || ENV["RUBYRS_COOP_THREADS"] != "0")
    end

    # True while at least one green thread is live — the single flag
    # every blocking-op hook checks before engaging the scheduler.
    def __coop_active?
      @coop_live > 0
    end

    # The green thread currently executing, nil on main.
    def __coop_current
      @coop_current
    end

    def __coop_runq
      @coop_runq
    end

    def __coop_now
      Time.now.to_f
    end

    def __coop_register(t)
      @coop_live += 1
      @coop_threads << t
      @coop_runq << t
    end

    def __coop_finish_thread(t)
      @coop_live -= 1
      @coop_threads.delete(t)
      t.__coop_wake_joiners
    end

    # Park bookkeeping: a thread is parked on exactly one waiter LIST
    # (an fd list, a join list, a queue/CV waiter list, a mutex waiter
    # list) and/or one timed SLEEPER entry. Wake = remove from both +
    # push onto the run queue.
    def __coop_make_runnable(t)
      return if t.__coop_done?
      t.__coop_clear_park
      @coop_runq << t unless @coop_runq.include?(t)
      nil
    end

    def __coop_add_sleeper(entry)
      @coop_sleepers << entry
      entry
    end

    def __coop_remove_sleeper(entry)
      @coop_sleepers.delete(entry)
      nil
    end

    # Green-thread suspension: yields to the scheduler; the resume arg
    # `:__coop_kill` means this thread was killed while parked — raise
    # so ensure blocks along the thread's stack run.
    #
    # When the park point sits under a NATIVE (Rust-driven) iterator
    # frame (`[..].each { Thread.pass }`, `n.times { q.pop }`, ...),
    # `Fiber.yield` cannot stash that frame and would silently
    # TRUNCATE the iteration (vm/iter.rs step_block's
    # fiber_yield_pending guard drops the remaining elements) —
    # observed as `[0,1,2].each { |i| p i; Thread.pass }` printing
    # only `0` inside a green thread. `__rubyrs_fiber_can_yield`
    # detects that shape (dispatch-nesting deeper than the fiber's
    # resume level); fall back to driving the scheduler INLINE, the
    # same way MAIN blocks. Kill delivered while inline-parked is
    # consumed from the resume-arg slot and raised here, mirroring
    # the resume-value path below.
    def __coop_yield_parked
      cur = @coop_current
      if cur && !__rubyrs_fiber_can_yield
        __coop_wait_inline(cur)
        raise CoopKill, "killed" if cur.__coop_take_kill_signal
        return nil
      end
      r = ::Fiber.yield
      raise CoopKill, "killed" if r == :__coop_kill
      r
    end

    # Inline-park fallback for a green thread that cannot
    # `Fiber.yield` (native Rust-driven frame between the fiber's
    # entry and the park point — see __coop_yield_parked). Drive the
    # scheduler ON TOP of the current stack until this thread is
    # WOKEN — i.e. until a waker / sleeper-expiry / kill calls
    # `__coop_make_runnable(cur)` and cur reaches the runq head.
    # FIFO is preserved: threads queued ahead of cur run first, so
    # `Thread.pass` under a native iterator (which requeues cur at
    # the TAIL before parking) still gives every runnable thread its
    # turn. Costs stack depth relative to a real switch, but is
    # semantically exact — no truncation.
    #
    # A runq entry can itself be inline-parked DEEPER on this very
    # stack (nested inline drives): its fiber is still Running, so
    # resuming it would be a FiberError double-resume. Its wake has
    # already fired — it proceeds when control unwinds back down —
    # so rotate past it. When EVERY queued thread is stack-pinned
    # that way, cur not woken, and nothing is pollable or timed,
    # the wake can only come from a pinned thread: a genuine stacked
    # deadlock. Raise ThreadError (loud) rather than hang.
    def __coop_wait_inline(cur)
      cur.__coop_inline_park_set(true)
      loop do
        head = @coop_runq.first
        if head.nil?
          # Nothing runnable: block in poll (fd parks, timed
          # sleepers — including cur's own sleeper entry). Poll's
          # "No live threads left. Deadlock?" fires when nothing
          # can wake FROM HERE — but unlike main's case this is
          # not always a program deadlock: the wake source may be
          # the very code pinned below this stack (e.g. main
          # producing into the queue cur pops inside `each`).
          # Either way this thread cannot proceed — re-raise with
          # the structural cause named. Documented Tier-1 limit;
          # the full fix is bytecode-level iteration (see
          # vm/iter.rs step_block).
          begin
            __coop_poll(nil, nil, nil)
          rescue ThreadError
            raise ThreadError,
                  "cannot suspend: thread is parked beneath a native iterator frame " \
                  "(e.g. `Array#each`/`Integer#times`) and no other thread is runnable — " \
                  "if the waker is the code below this stack, move the blocking call " \
                  "out of the iterator (use a `while` loop)"
          end
          next
        end
        if head.equal?(cur)
          @coop_runq.shift
          break
        end
        if head.__coop_done?
          @coop_runq.shift
          next
        end
        if head.__coop_inline_parked?
          @coop_runq.push(@coop_runq.shift)
          # while-scan, not include?/any? — no native-iterator
          # frames inside the inline drive (see __coop_poll).
          cur_queued = false
          runnable = false
          i = 0
          rq = @coop_runq
          while i < rq.length
            cur_queued = true if rq[i].equal?(cur)
            runnable = true unless rq[i].__coop_inline_parked?
            i += 1
          end
          next if cur_queued
          unless runnable
            if @coop_fd_r.empty? && @coop_fd_w.empty? && @coop_sleepers.empty?
              raise ThreadError,
                    "deadlock: every runnable thread is parked beneath a native iterator frame"
            end
            __coop_poll(nil, nil, nil)
          end
          next
        end
        @coop_runq.shift
        prev = @coop_current
        @coop_current = head
        begin
          head.__coop_run
        ensure
          @coop_current = prev
        end
      end
    ensure
      cur.__coop_inline_park_set(false)
    end

    # One scheduler step: resume the next runnable green thread, or —
    # with nothing runnable — block in poll(2) over the parked fds
    # (bounded by the earliest timed wake / caller deadline).
    # `main_fd`/`main_dir` name the fd MAIN itself is blocked on, if
    # any, so the poll includes it.
    def __coop_schedule_step(main_fd, main_dir, deadline)
      t = @coop_runq.shift
      unless t
        __coop_poll(main_fd, main_dir, deadline)
        return
      end
      return if t.__coop_done? # killed while queued
      prev = @coop_current
      @coop_current = t
      begin
        t.__coop_run
      ensure
        @coop_current = prev
      end
      nil
    end

    # `while`-loop iteration throughout (not Array#each): this runs at
    # the bottom of every park, including a green thread's INLINE park
    # (`__coop_wait_inline`), where the stack already carries native
    # iterator frames. Native (Rust-driven) iterators here would both
    # deepen the re-entrant dispatch nesting (tripping the tight
    # debug-profile dispatch cap on shapes like `each { sleep 0 }`)
    # and violate this file's "park machinery reaches the scheduler
    # through pure-Ruby frames only" rule.
    def __coop_poll(main_fd, main_dir, deadline)
      rfds = @coop_fd_r.keys
      wfds = @coop_fd_w.keys
      if main_fd
        (main_dir == :w ? wfds : rfds) << main_fd
      end
      # Earliest timed wake bounds the poll.
      min_wake = deadline
      i = 0
      sleepers = @coop_sleepers
      while i < sleepers.length
        wake = sleepers[i][0]
        min_wake = wake if wake && (min_wake.nil? || wake < min_wake)
        i += 1
      end
      timeout_ms =
        if min_wake
          ms = ((min_wake - __coop_now) * 1000).ceil
          ms < 0 ? 0 : ms
        else
          -1
        end
      if rfds.empty? && wfds.empty? && timeout_ms == -1
        # Nothing runnable, nothing pollable, nothing timed: every
        # remaining thread waits on another thread forever. CRuby
        # aborts with fatal here; ThreadError is our catchable twin.
        raise ThreadError, "No live threads left. Deadlock?"
      end
      ready = __rubyrs_fd_poll(rfds, wfds, timeout_ms)
      i = 0
      rready = ready[0]
      while i < rready.length
        ts = @coop_fd_r.delete(rready[i])
        if ts
          ts = ts.dup
          j = 0
          while j < ts.length
            __coop_make_runnable(ts[j])
            j += 1
          end
        end
        i += 1
      end
      i = 0
      wready = ready[1]
      while i < wready.length
        ts = @coop_fd_w.delete(wready[i])
        if ts
          ts = ts.dup
          j = 0
          while j < ts.length
            __coop_make_runnable(ts[j])
            j += 1
          end
        end
        i += 1
      end
      unless @coop_sleepers.empty?
        now = __coop_now
        expired = @coop_sleepers.dup
        i = 0
        while i < expired.length
          wake = expired[i][0]
          __coop_make_runnable(expired[i][1]) if wake && wake <= now
          i += 1
        end
      end
      nil
    end

    # Block the CALLING thread until `fd` is ready for `dir` (:r/:w).
    # Green thread: park on the fd table + yield. Main: drive the
    # scheduler (resuming runnable threads between readiness probes;
    # blocking in poll with the fd included when nothing is runnable).
    def __coop_wait_fd(fd, dir)
      cur = @coop_current
      if cur
        table = dir == :w ? @coop_fd_w : @coop_fd_r
        lst = (table[fd] ||= [])
        lst << cur
        cur.__coop_set_park(lst, nil)
        __coop_yield_parked
      else
        loop do
          pr =
            if dir == :w
              __rubyrs_fd_poll([], [fd], 0)
            else
              __rubyrs_fd_poll([fd], [], 0)
            end
          break unless (dir == :w ? pr[1] : pr[0]).empty?
          __coop_schedule_step(fd, dir, nil)
        end
      end
      nil
    end

    # Timed / untimed suspension. Green thread: sleeper-park. Main
    # (only reached with live green threads): drive the scheduler for
    # the duration. `secs = nil` parks forever (until kill/wakeup).
    def __coop_sleep(secs)
      cur = @coop_current
      if cur
        wake = secs ? __coop_now + secs : nil
        entry = [wake, cur]
        @coop_sleepers << entry
        cur.__coop_set_park(nil, entry)
        __coop_yield_parked
      else
        return unless secs
        deadline = __coop_now + secs
        while __coop_now < deadline
          __coop_schedule_step(nil, nil, deadline)
        end
      end
      nil
    end

    # Park the calling thread on an arbitrary waiter list (Queue, CV,
    # Mutex) with an optional deadline. Main blocks via a token entry
    # — its removal from the list (by a waker) is the wake signal.
    def __coop_block(list, deadline)
      cur = @coop_current
      if cur
        entry = nil
        if deadline
          entry = [deadline, cur]
          @coop_sleepers << entry
        end
        list << cur
        cur.__coop_set_park(list, entry)
        __coop_yield_parked
      else
        token = ::Object.new
        list << token
        while list.include?(token)
          if deadline && __coop_now >= deadline
            list.delete(token)
            break
          end
          __coop_schedule_step(nil, nil, deadline)
        end
      end
      nil
    end

    # Wake one waiter off a list (FIFO). Skips dead threads; a
    # non-Thread entry is a parked MAIN token whose removal is its
    # wake. Returns true when something was woken.
    def __coop_wake_one(list)
      while (w = list.shift)
        if w.is_a?(::Thread)
          next if w.__coop_done?
          __coop_make_runnable(w)
          return true
        else
          return true
        end
      end
      false
    end

    def __coop_wake_all(list)
      woke = false
      woke = true while __coop_wake_one(list)
      woke
    end

    # POSIX fork: only the forking thread survives in the child. Reset
    # the scheduler world — parent green threads must never be resumed
    # here, parent-parked fds never polled.
    def __coop_after_fork!
      @coop_live = 0
      @coop_current = nil
      @coop_threads = []
      @coop_runq = []
      @coop_fd_r = {}
      @coop_fd_w = {}
      @coop_sleepers = []
      nil
    end

    # `Thread.pass` — cooperative reschedule point. A green thread
    # requeues itself and yields; main runs one runnable thread (never
    # blocks) if any.
    def pass
      cur = @coop_current
      if cur
        @coop_runq << cur
        __coop_yield_parked
      elsif @coop_live > 0 && !@coop_runq.empty?
        __coop_schedule_step(nil, nil, nil)
      end
      nil
    end
  end

  # ---- cooperative Thread instances ----------------------------------

  def __coop_init(args, block)
    @coop = true
    @args = args
    @block = block
    @done = false
    @started = false
    @killed = false
    @value = nil
    @exception = nil
    @resume_arg = nil
    @park_ref = nil
    @sleep_ref = nil
    @inline_parked = false
    @joiners = []
    @fiber_locals = {}
    ::Thread.__coop_register(self)
    self
  end

  def __coop_done?
    @done
  end

  # True while this thread is blocked in `__coop_wait_inline` — its
  # fiber is Running with live frames pinned on the physical stack,
  # so it must NOT be fiber-resumed (double resume); it continues on
  # its own when control unwinds back to its inline loop.
  def __coop_inline_parked?
    @inline_parked
  end

  def __coop_inline_park_set(v)
    @inline_parked = v
    nil
  end

  # Consume a kill delivered while this thread was inline-parked
  # (`__coop_kill` on a parked thread stores `:__coop_kill` in the
  # resume-arg slot; the fiber path delivers it as the resume VALUE,
  # the inline path reads it here).
  def __coop_take_kill_signal
    if @resume_arg == :__coop_kill
      @resume_arg = nil
      true
    else
      false
    end
  end

  def __coop_set_park(list_ref, sleeper_entry)
    @park_ref = list_ref
    @sleep_ref = sleeper_entry
    nil
  end

  def __coop_clear_park
    @park_ref&.delete(self)
    @park_ref = nil
    if @sleep_ref
      ::Thread.__coop_remove_sleeper(@sleep_ref)
      @sleep_ref = nil
    end
    nil
  end

  def __coop_wake_joiners
    js = @joiners
    return if js.nil? || js.empty?
    js.dup.each do |j|
      if j.is_a?(::Thread)
        ::Thread.__coop_make_runnable(j)
      else
        js.delete(j) # main token: removal is the wake
      end
    end
    js.clear
    nil
  end

  # Invoke the thread body through Op::Yield (flat, same dispatch
  # level) rather than `@block.call` (a re-entrant dispatch_until
  # level). Keeps the body's park points at EXACTLY the fiber's
  # resume-level dispatch depth, so `__rubyrs_fiber_can_yield`'s
  # depth comparison distinguishes "plain body frames" (yieldable)
  # from "pinned under a native iterator" (inline-drive fallback) —
  # and the yield path no longer unwinds across a Proc#call re-entry
  # at all.
  def __coop_call_body
    yield(*@args)
  end

  # Scheduler-side resume of this thread (runs with @coop_current set).
  def __coop_run
    unless @started
      @started = true
      @fiber = ::Fiber.new do
        begin
          @value = __coop_call_body(&@block)
        rescue ::Thread::CoopKill
          @killed = true
        rescue ::Exception => e
          @exception = e
        end
      end
    end
    arg = @resume_arg
    @resume_arg = nil
    @fiber.resume(arg)
    if @fiber.alive?
      # Yielded. Normally the park point registered itself before
      # yielding; a bare `Fiber.yield` from thread code (no park)
      # degrades to Thread.pass — keep the thread runnable rather
      # than losing it.
      if @park_ref.nil? && @sleep_ref.nil? && !::Thread.__coop_runq.include?(self)
        ::Thread.__coop_runq << self
      end
    else
      @done = true
      ::Thread.__coop_finish_thread(self)
    end
    nil
  end

  def __coop_join(limit)
    cur = ::Thread.__coop_current
    raise ThreadError, "Target thread must not be current thread" if cur.equal?(self)
    unless @done
      if cur
        entry = nil
        if limit
          entry = [::Thread.__coop_now + limit, cur]
          ::Thread.__coop_add_sleeper(entry)
        end
        @joiners << cur
        cur.__coop_set_park(@joiners, entry)
        ::Thread.__coop_yield_parked
        return nil unless @done # timed out
      elsif limit
        deadline = ::Thread.__coop_now + limit
        until @done
          break if ::Thread.__coop_now >= deadline
          ::Thread.__coop_schedule_step(nil, nil, deadline)
        end
        return nil unless @done
      else
        ::Thread.__coop_schedule_step(nil, nil, nil) until @done
      end
    end
    raise @exception if @exception
    self
  end

  def __coop_kill
    return self if @done
    if !@started
      # Never scheduled: it never runs at all (same shape CRuby shows
      # for a kill that lands before the thread got the CPU, and the
      # deferred model's documented kill semantics).
      @done = true
      @killed = true
      ::Thread.__coop_finish_thread(self)
    elsif ::Thread.__coop_current.equal?(self)
      raise ::Thread::CoopKill, "killed"
    else
      @resume_arg = :__coop_kill
      ::Thread.__coop_make_runnable(self)
    end
    self
  end

  # CRuby Thread instance surface used by real code: fiber-locals
  # (`Thread.current[:k]` inside a green thread lands here), thread
  # variables, wakeup/run, name.
  def [](key)
    (@fiber_locals ||= {})[key]
  end

  def []=(key, val)
    (@fiber_locals ||= {})[key] = val
  end

  def key?(key)
    (@fiber_locals ||= {}).key?(key)
  end

  def thread_variable_get(key)
    (@thread_vars ||= {})[key]
  end

  def thread_variable_set(key, val)
    (@thread_vars ||= {})[key] = val
  end

  def thread_variable?(key)
    (@thread_vars ||= {}).key?(key)
  end

  def wakeup
    raise ThreadError, "killed thread" if @done
    if @coop && @started && (@park_ref || @sleep_ref)
      ::Thread.__coop_make_runnable(self)
    end
    self
  end
  alias run wakeup

  def name
    @name
  end

  def name=(v)
    @name = v
  end

  def inspect
    st = status
    st = st.nil? ? "aborting" : (st == false ? "dead" : st)
    "#<Thread:0x#{object_id.to_s(16)} #{st}>"
  end
end

# `Kernel#sleep` becomes a scheduler yield point. The native builtin
# ("sleep" arm in vm/kernel.rs) checks self's class chain for an
# override and finds THIS method; the non-scheduler branch escapes to
# the native through the `__rubyrs_kernel_sleep` spelling (same arm,
# override check skipped) so there is no recursion. Private like
# CRuby's Kernel#sleep — explicit-receiver `obj.sleep` stays a
# NoMethodError.
class Object
  def sleep(*secs)
    if ::Thread.__coop_active?
      s = secs[0]
      ::Thread.__coop_sleep(s)
      s ? s.to_i : nil
    else
      __rubyrs_kernel_sleep(*secs)
    end
  end
  private :sleep
end
