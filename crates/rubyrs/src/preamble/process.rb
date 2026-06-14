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

  # `IO.pipe` — an in-memory pipe pair. Tier-1 models the
  # single-threaded write-then-read shape (rack's MockRequest
  # specs feed a pipe as rack.input): the writer appends to a
  # shared binary buffer, the reader consumes byte-based with the
  # `(length, outbuf)` IO contract. No real fd, no blocking
  # semantics — a reader that outpaces the writer sees EOF
  # immediately rather than blocking (documented divergence; the
  # cross-thread streaming shape needs real pipes).
  def self.pipe
    state = { buf: +"".b, pos: 0, wclosed: false }
    r = RubyrsPipeReader.new(state)
    w = RubyrsPipeWriter.new(state)
    if block_given?
      begin
        return yield(r, w)
      ensure
        r.close rescue nil
        w.close rescue nil
      end
    end
    [r, w]
  end

  def write(*args)
    # Delegation mode — `$stdout.reopen(tempfile)` redirects this
    # handle's writes into the target until a reopen back to a real
    # IO (minitest's capture_subprocess_io). See #reopen.
    if @delegate
      total = 0
      args.each { |a| total += @delegate.write(a.to_s).to_i }
      return total
    end
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

  # `io.reopen(target)` — CRuby redirects the underlying fd. Tier-1
  # models the observable subset: writes DELEGATE to the target
  # (a Tempfile / StringIO-ish) until reopened back onto a real IO
  # (the `$stdout.reopen orig_stdout` restore half — orig is a
  # `$stdout.dup`, itself an IO veneer, which clears delegation).
  # minitest's capture_subprocess_io is the round-trip consumer.
  def reopen(target)
    @delegate = target.is_a?(IO) ? nil : target
    self
  end

  # Forward rewind to the delegate (capture_subprocess_io rewinds
  # $stdout before reading the tempfile back); no-op on a native
  # sink.
  def rewind
    @delegate ? @delegate.rewind : 0
  end

  def read
    @delegate ? @delegate.read : nil
  end

  def close
    nil
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

# `Kernel#system` lives in the Rust builtin table (vm/kernel.rs)
# behind the `allow_process_spawn` capability — nil when the
# capability is off (the old Tier-1 constant answer), real
# subprocess execution when the host opts in (the CLI does).

# Warning — CRuby's warning-control module (Warning.warn override
# point, Warning[]= category toggles). Tier-1 ships the bare
# module so feature probes (`::Warning.respond_to? :[]=` —
# minitest's process_args does this at option-build time) resolve
# the constant and answer false; rubyrs has no warning categories
# to toggle.
module Warning
end

# Kernel#fork / Process.fork / Process.waitpid — REAL fork(2),
# gated on the same process-spawn capability as Kernel#system
# (ADR 0017) and on a unix host. Where unsupported the methods
# are simply NOT DEFINED, so `Process.respond_to?(:fork)` is
# false exactly like CRuby on Windows — minitest's fork-exit
# tests then take their documented skip path.
#
# Subset: block form only (`fork { ... }`); the no-block
# both-sides-continue form would need the dispatch loop itself to
# be re-entered post-fork and is declined loudly.
if __rubyrs_fork_supported?
  module Kernel
    def fork(&blk)
      raise NotImplementedError, "rubyrs fork requires a block (Tier-1 subset)" unless blk
      __rubyrs_fork_block(blk)
    end
  end

  module Process
    def self.fork(&blk)
      raise NotImplementedError, "rubyrs fork requires a block (Tier-1 subset)" unless blk
      __rubyrs_fork_block(blk)
    end

    def self.waitpid(pid, flags = 0)
      r = __rubyrs_waitpid(pid, flags)
      $? = Process::Status.new(r[0], r[1])
      r[0]
    end

    class << self
      alias_method :wait, :waitpid
    end
  end
end

module Process
  # `$?` value shape — only the surface minitest / shell-status
  # consumers read (exitstatus / success? / pid). CRuby packs the
  # wait(2) status word; `to_i` reproduces the exited-child form.
  class Status
    attr_reader :pid, :exitstatus

    def initialize(pid, exitstatus)
      @pid = pid
      @exitstatus = exitstatus
    end

    def success?
      @exitstatus == 0
    end

    def exited?
      true
    end

    def signaled?
      false
    end

    def to_i
      @exitstatus << 8
    end

    def inspect
      "#<Process::Status: pid #{@pid} exit #{@exitstatus}>"
    end
    alias_method :to_s, :inspect
  end
end

# IO.pipe's in-memory endpoints (see IO.pipe above). Both are
# byte-based over one shared state Hash; deliberately NOT IO
# subclasses (IO#initialize models stdout/stderr sinks) — duck-typed
# like StringIO, which is what rack/minitest consumers dispatch on.
class RubyrsPipeReader
  def initialize(state)
    @state = state
    @closed = false
  end

  def read(length = nil, outbuf = nil)
    raise IOError, "closed stream" if @closed
    buf = @state[:buf]
    total = buf.bytesize
    pos = @state[:pos]
    result =
      if length.nil?
        out = buf.byteslice(pos, total - pos) || ""
        @state[:pos] = total
        out
      else
        chunk = buf.byteslice(pos, length) || ""
        @state[:pos] = pos + chunk.bytesize
        if chunk.bytesize == 0 && length > 0
          # Empty buffer. While the write end is still OPEN this is
          # "no data available yet", NOT end-of-stream — a real pipe
          # blocks here. The in-memory shim can't block (single-thread,
          # ADR 0017), but returning "" rather than nil keeps emptiness
          # distinct from EOF: a reader that declared a content-length
          # (rack multipart's BoundedIO) then sees empty *content* and
          # raises EmptyContentError, instead of mistaking it for a
          # truncated body (EOFError). Once the writer closes, an empty
          # buffer is genuine EOF → nil.
          @state[:wclosed] ? nil : ""
        else
          chunk
        end
      end
    if outbuf
      outbuf.replace(result || "")
      result.nil? ? nil : outbuf
    else
      result
    end
  end

  def gets(sep = "\n")
    buf = @state[:buf]
    total = buf.bytesize
    pos = @state[:pos]
    return nil if pos >= total
    idx = buf.byteindex(sep, pos)
    if idx
      line = buf.byteslice(pos, idx + sep.bytesize - pos)
      @state[:pos] = idx + sep.bytesize
    else
      line = buf.byteslice(pos, total - pos)
      @state[:pos] = total
    end
    line
  end

  def each(sep = "\n")
    while (l = gets(sep))
      yield l
    end
    self
  end
  alias_method :each_line, :each

  def eof?
    @state[:pos] >= @state[:buf].bytesize
  end

  def rewind
    @state[:pos] = 0
    0
  end

  def binmode; self; end
  def set_encoding(*_a); self; end

  def close
    @closed = true
    # Signal the write end: further writes must fail with EPIPE, the way
    # a real pipe behaves once its read end is gone (rack's multipart
    # "rejects insanely long boundaries" test relies on this to unblock
    # the producer thread after Rack shuts the reader down).
    @state[:rclosed] = true
    nil
  end

  def closed?
    @closed
  end
end

class RubyrsPipeWriter
  def initialize(state)
    @state = state
    @closed = false
  end

  def write(*args)
    raise IOError, "closed stream" if @closed
    # Writing to a pipe whose read end has been closed is Errno::EPIPE
    # in CRuby — surface the same so producers terminate (and rescue it)
    # instead of buffering forever.
    raise Errno::EPIPE, "Broken pipe" if @state[:rclosed]
    total = 0
    args.each do |a|
      s = a.to_s
      @state[:buf] << s.b
      total += s.bytesize
    end
    total
  end

  # Non-blocking write. The in-memory buffer never actually blocks, so a
  # write always completes — except against a closed read end, where
  # CRuby raises Errno::EPIPE even with `exception: false` (that flag
  # only suppresses EAGAIN/EWOULDBLOCK, never EPIPE).
  def write_nonblock(s, exception: true)
    raise IOError, "closed stream" if @closed
    raise Errno::EPIPE, "Broken pipe" if @state[:rclosed]
    str = s.to_s
    @state[:buf] << str.b
    str.bytesize
  end

  def <<(s)
    write(s)
    self
  end

  def puts(*args)
    if args.empty?
      write("\n")
    else
      args.each do |a|
        s = a.to_s
        write(s)
        write("\n") unless s.end_with?("\n")
      end
    end
    nil
  end

  def print(*args)
    args.each { |a| write(a.to_s) }
    nil
  end

  def flush; self; end
  def sync; true; end
  def sync=(_v); _v; end
  def binmode; self; end
  def set_encoding(*_a); self; end

  def close
    @closed = true
    @state[:wclosed] = true
    nil
  end

  def closed?
    @closed
  end
end
