# Mutex — real ownership tracking, cooperative blocking.
#
# Historically this was a pure no-op shim (single-threaded world:
# "pretending to lock is fine when there's nothing to lock against").
# With the fiber-backed cooperative green-thread scheduler
# (preamble/thread.rb), Mutex now tracks a real owner and a CONTENDED
# `lock` parks the calling thread until release — the parallel gem's
# supervisor threads serialize their job-index pops and result stores
# through one Mutex.
#
# Single-threaded fast path: with no green threads live, `lock` is
# "set owner", `unlock` is "clear owner" — no scheduler interaction,
# no waiters. Contention is impossible with one thread.
#
# Retained lenient divergences (documented, deliberate):
#   - Re-entrant `lock`/`synchronize` by the OWNING thread is a
#     counted no-op instead of CRuby's ThreadError("deadlock;
#     recursive locking"). Code written against the historical shim
#     relies on this; the divergence is in the user-friendly
#     direction and cross-thread exclusion still holds.
#   - `unlock` by a non-owner is a no-op instead of ThreadError.
class Mutex
  # CRuby's Mutex.new takes zero args; the explicit 0-arity
  # initialize delegates arity-checking to the method-call machinery
  # so `Mutex.new(1)` raises ArgumentError.
  def initialize
    @owner = nil
    @depth = 0
    @waiters = []
  end

  def synchronize
    lock
    begin
      yield
    ensure
      unlock
    end
  end

  def lock
    cur = ::Thread.current
    if @owner.nil?
      @owner = cur
    elsif @owner.equal?(cur)
      @depth += 1
    else
      # Contended — only possible with live green threads. Park until
      # the owner releases; re-check on wake (another waiter may have
      # grabbed it first).
      until @owner.nil? || @owner.equal?(cur)
        ::Thread.__coop_block(@waiters, nil)
      end
      @owner = cur if @owner.nil?
    end
    self
  end

  def unlock
    cur = ::Thread.current
    if @owner.equal?(cur)
      if @depth > 0
        @depth -= 1
      else
        @owner = nil
        ::Thread.__coop_wake_one(@waiters) unless @waiters.empty?
      end
    end
    self
  end

  def try_lock
    cur = ::Thread.current
    if @owner.nil?
      @owner = cur
      true
    else
      # CRuby: false when already locked — including by self.
      false
    end
  end

  def locked?
    !@owner.nil?
  end

  def owned?
    @owner.equal?(::Thread.current)
  end
end
