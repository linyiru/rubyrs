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

# ---- Standard streams ----------------------------------------
#
# CRuby's $stdout/$stderr/STDOUT/STDERR are IO objects; rubyrs's
# write path is the host-injected sink pair on the Vm
# (`set_stdout`/`set_stderr`), so these IO instances are thin Ruby
# veneers over the `__rubyrs_std*_write` raw-byte builtins.
#
# DIVERGENCE: Kernel#puts/print write to the VM sink DIRECTLY —
# reassigning `$stdout = StringIO.new` redirects only calls made
# through the $stdout object, not bare puts. (CRuby consults the
# global dynamically.) Test frameworks pass the IO object around
# explicitly (minitest's `Minitest.io`), which is the supported
# shape. STDIN stays absent-loud: rubyrs has no input capability.
class IO
  def initialize(which)
    @which = which
  end

  def write(*args)
    total = 0
    args.each do |a|
      s = a.to_s
      total += s.bytesize
      if @which == :err
        __rubyrs_stderr_write(s)
      else
        __rubyrs_stdout_write(s)
      end
    end
    total
  end

  def <<(obj)
    write(obj.to_s)
    self
  end

  def print(*args)
    args.each { |a| write(a.to_s) }
    nil
  end

  def puts(*args)
    if args.empty?
      write("\n")
    else
      args.each do |a|
        if a.is_a?(Array)
          # CRuby: an empty Array contributes NO output (not even
          # a bare newline).
          a.each { |x| puts(x) }
        else
          s = a.to_s
          write(s.end_with?("\n") ? s : "#{s}\n")
        end
      end
    end
    nil
  end

  def printf(fmt, *args)
    write(format(fmt, *args))
    nil
  end

  def flush
    self
  end

  # CRuby's piped-stdout default; assignment is honoured for
  # read-back but write behavior is unchanged (the VM sink flushes
  # on its own schedule).
  def sync
    @sync = false if @sync.nil?
    @sync
  end

  def sync=(v)
    @sync = v
  end

  def tty?
    false
  end
  alias isatty tty?

  def fileno
    @which == :err ? 2 : 1
  end

  def closed?
    false
  end
end

STDOUT = IO.new(:out)
STDERR = IO.new(:err)
$stdout = STDOUT
$stderr = STDERR

# ARGV — empty by default (deterministic library posture); the CLI
# overwrites it with the post-script-path arguments via
# `Runtime::set_argv`.
ARGV = []

# RbConfig — CRuby's build-configuration table, available without a
# require. rubyrs has no autoconf build, so the table carries one
# honest sentinel: host_os = "rubyrs" (consumers branch on
# /mswin|mingw/-style probes — minitest's diff-tool discovery — and
# a neutral value routes them down the POSIX path). Other keys are
# absent-loud: a script needing "bindir" etc. should fail visibly
# rather than act on an invented path.
module RbConfig
  CONFIG = { "host_os" => "rubyrs" }
end

# `Kernel#system` — process spawning is host-side per ADR 0017
# Rule 4; the Tier-1 answer is nil, CRuby's "command could not be
# executed" shape, which probing callers (minitest's
# `system "diff", ...` tool discovery) treat as feature-absent and
# route around. DIVERGENCE: a script that NEEDS the side effect of
# a real command sees nil instead of the command running.
def system(*_args)
  nil
end

# Warning — CRuby's warning-control module (Warning.warn override
# point, Warning[]= category toggles). Tier-1 ships the bare
# module so feature probes (`::Warning.respond_to? :[]=` —
# minitest's process_args does this at option-build time) resolve
# the constant and answer false; rubyrs has no warning categories
# to toggle.
module Warning
end
