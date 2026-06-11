# Process — the host-process reflection surface. rubyrs is a
# single-process, single-threaded runtime, so most of CRuby's
# Process API (fork/wait/spawn/kill) is out of scope (ADR 0017
# Rule 4 keeps process control host-side); what lives here is the
# read-only subset real gems consult at load/run time.
#
# Motivating consumer: minitest 5.25 — `Process.pid` anchors the
# at_exit guard (fork detection; with no fork the guard is always
# true) and `Process.clock_gettime(Process::CLOCK_MONOTONIC)` times
# the run.
module Process
  # Clock ids — values mirror Linux's clockid_t numbering; rubyrs
  # only distinguishes them nominally (see clock_gettime).
  CLOCK_REALTIME = 0
  CLOCK_MONOTONIC = 1
  CLOCK_PROCESS_CPUTIME_ID = 2
  CLOCK_THREAD_CPUTIME_ID = 3

  # `$$` is the Config::pid capability (0 when the host didn't
  # inject one — wasi, sandboxed embedders). Same value, method
  # spelling.
  def self.pid
    $$
  end

  # DIVERGENCE: every clock id reads the injected wall clock
  # (`Config::time_now`, same capability Time.now uses) — there is
  # no separate monotonic source, so this clock can jump if the
  # host clock does. Callers measure short test-run durations;
  # fail-loud capability behavior (raises without time_now, like
  # Time.now) is inherited rather than masked.
  def self.clock_gettime(_clock_id, unit = :float_second)
    t = Time.now
    case unit
    when :float_second then t.to_f
    when :float_millisecond then t.to_f * 1000.0
    when :float_microsecond then t.to_f * 1_000_000.0
    when :second then t.to_i
    when :millisecond then (t.to_f * 1000.0).to_i
    when :microsecond then (t.to_f * 1_000_000.0).to_i
    when :nanosecond then (t.to_f * 1_000_000_000.0).to_i
    else
      raise ArgumentError, "unexpected unit: #{unit}"
    end
  end
end
