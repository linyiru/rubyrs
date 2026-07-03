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
  # `seek`/`sysseek` whence constants (CRuby values). `File#seek`
  # already interprets 0/1/2 (preamble/file.rb); these named
  # constants let gems pass `IO::SEEK_SET` explicitly — mini_mime's
  # `pread` does `@file.seek(offset, IO::SEEK_SET)`.
  SEEK_SET = 0
  SEEK_CUR = 1
  SEEK_END = 2

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
  # `IO.readlines(path, ...)` / `IO.foreach(path, ...)` — File < IO in
  # CRuby, so these line readers are IO class methods File inherits.
  # rubyrs doesn't model File < IO (File.superclass is Object), so
  # delegate to the File veneer, which owns the buffered read primitive.
  # Block / Enumerator / sep / `chomp:` semantics all ride along.
  def self.readlines(path, *args, **opts, &blk)
    File.readlines(path, *args, **opts, &blk)
  end

  def self.foreach(path, *args, **opts, &blk)
    File.foreach(path, *args, **opts, &blk)
  end

  def self.pipe
    # Two backings, one API:
    #   - REAL fd pipe(2) (RubyrsFdReader/Writer) when the host can
    #     fork — pipe endpoints must survive fork(2) and carry real
    #     blocking/EOF/EPIPE semantics for the cross-process protocol
    #     the parallel gem runs (rubocop --parallel's Marshal frames).
    #   - the in-memory shim otherwise (embedded/wasi hosts keep the
    #     documented single-threaded write-then-read divergences).
    if __rubyrs_fork_supported?
      fds = __rubyrs_pipe
      r = RubyrsFdReader.new(fds[0])
      w = RubyrsFdWriter.new(fds[1])
    else
      state = { buf: +"".b, pos: 0, wclosed: false }
      r = RubyrsPipeReader.new(state)
      w = RubyrsPipeWriter.new(state)
    end
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
  # host_os = "rubyrs": consumers branch on /mswin|mingw/-style probes
  # (minitest's diff-tool discovery) and a neutral value routes them
  # down the POSIX path. The interpreter-path keys are derived from the
  # REAL running executable (`__rubyrs_exe_path`) so they're honest
  # rather than invented — rake's file_utils.rb computes
  # `RUBY = File.join(bindir, ruby_install_name + EXEEXT)` at load, and
  # rubyrs IS the interpreter that `RUBY` should point at. When the OS
  # can't report the exe path, fall back to a bare "rubyrs" name (still
  # non-nil, so the load-time `+`/`File.join` arithmetic doesn't crash).
  __exe = (__rubyrs_exe_path rescue nil)
  CONFIG = {
    "host_os"           => "rubyrs",
    "EXEEXT"            => "",
    "bindir"            => (__exe ? File.dirname(__exe) : "."),
    "ruby_install_name" => (__exe ? File.basename(__exe) : "rubyrs"),
    "RUBY_INSTALL_NAME" => (__exe ? File.basename(__exe) : "rubyrs"),
  }

  # `RbConfig.ruby` — full path to the running interpreter (CRuby
  # exposes this; rake/test tooling calls it). Honest exe path, or the
  # bare name as a last resort.
  def self.ruby
    __exe = (__rubyrs_exe_path rescue nil)
    __exe || "rubyrs"
  end
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
      # POSIX: only the forking thread survives in the child — the
      # cooperative scheduler's world (parent green threads, parked
      # fds, run queue) must reset before the child's block runs, or
      # the child could resume parent supervisors / double-poll the
      # parent's pipe fds. (The Rust fork arm clears the VM-side fiber
      # state; this clears the Ruby-side tables.)
      __rubyrs_fork_block(proc {
        ::Thread.__coop_after_fork!
        blk.call
      })
    end
  end

  module Process
    # waitpid(2) flags (CRuby values).
    WNOHANG = 1
    WUNTRACED = 2

    def self.fork(&blk)
      raise NotImplementedError, "rubyrs fork requires a block (Tier-1 subset)" unless blk
      __rubyrs_fork_block(proc {
        ::Thread.__coop_after_fork!
        blk.call
      })
    end

    def self.waitpid(pid, flags = 0)
      # Cooperative scheduling: a blocking wait from a green thread
      # (the parallel gem's Worker#stop in each supervisor's ensure)
      # must not stall the whole VM — poll with WNOHANG and park
      # between attempts so the other supervisors keep running.
      if flags == 0 && ::Thread.__coop_active?
        loop do
          r = __rubyrs_waitpid(pid, WNOHANG)
          if r
            $? = Process::Status.new(r[0], r[1])
            return r[0]
          end
          ::Thread.__coop_sleep(0.002)
        end
      end
      r = __rubyrs_waitpid(pid, flags)
      return nil if r.nil? # WNOHANG, child still running
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

# Real-fd IO.pipe endpoints (see IO.pipe above) — thin veneers over
# the `__rubyrs_fd_*` host primitives (raw pipe(2) fds). Unlike the
# in-memory shim these carry REAL pipe semantics: reads BLOCK until
# data or writer-close (EOF), writes to a reader-less pipe raise
# Errno::EPIPE, and the fds survive fork(2) — which is the point:
# the parallel gem's work_in_processes protocol (rubocop --parallel)
# Marshal-frames jobs/results across a fork boundary. Duck-typed like
# StringIO rather than IO subclasses, same as the in-memory pair.
class RubyrsFdReader
  def initialize(fd)
    @fd = fd
    @closed = false
    # Pushback buffer: `eof?` on a pipe must BLOCK until it can give a
    # definitive answer, which costs one speculative byte — stash it
    # here for the next read. (CRuby's own IO#eof? does exactly this.)
    @pb = +"".b
  end

  def fileno
    @fd
  end

  def read(length = nil, outbuf = nil)
    raise IOError, "closed stream" if @closed
    result =
      if length.nil?
        rest = ::Thread.__coop_active? ? __coop_read_all : __rubyrs_fd_read(@fd, nil)
        out = @pb + (rest || "".b)
        @pb = +"".b
        out
      elsif length == 0
        "".b
      else
        have = @pb.bytesize
        if have >= length
          out = @pb.byteslice(0, length)
          @pb = @pb.byteslice(length, have - length) || +"".b
          out
        else
          rest =
            if ::Thread.__coop_active?
              __coop_read_exact(length - have)
            else
              __rubyrs_fd_read(@fd, length - have)
            end
          if rest.nil?
            # EOF before any fresh byte: drain the pushback if there
            # is one, else the nil-at-EOF contract.
            if have == 0
              nil
            else
              out = @pb
              @pb = +"".b
              out
            end
          else
            out = @pb + rest
            @pb = +"".b
            out
          end
        end
      end
    if outbuf
      outbuf.replace(result || "")
      result.nil? ? nil : outbuf
    else
      result
    end
  end

  # Cooperative twins of the blocking `__rubyrs_fd_read` shapes: same
  # exactly-n / to-EOF contracts, but a would-block read PARKS the
  # calling green thread on the fd (or drives the scheduler when main
  # is the caller) instead of blocking the whole VM in read(2). The
  # single-threaded path above never comes through here — zero cost.
  def __coop_read_exact(n)
    buf = +"".b
    while buf.bytesize < n
      chunk = __rubyrs_fd_read_step(@fd, n - buf.bytesize)
      if chunk == false
        ::Thread.__coop_wait_fd(@fd, :r)
      elsif chunk.nil?
        break # EOF
      else
        buf << chunk
      end
    end
    buf.empty? && n > 0 ? nil : buf
  end

  def __coop_read_all
    buf = +"".b
    loop do
      chunk = __rubyrs_fd_read_step(@fd, 65536)
      if chunk == false
        ::Thread.__coop_wait_fd(@fd, :r)
      elsif chunk.nil?
        break
      else
        buf << chunk
      end
    end
    buf
  end

  def getbyte
    b = read(1)
    b && b.getbyte(0)
  end

  # Byte-at-a-time line read — correctness over throughput (a pipe
  # can't over-read without a pushback discipline; consumers here
  # read short protocol lines).
  def gets(sep = "\n")
    raise IOError, "closed stream" if @closed
    line = +"".b
    loop do
      c = read(1)
      if c.nil?
        return line.empty? ? nil : line
      end
      line << c
      return line if line.end_with?(sep)
    end
  end

  def each(sep = "\n")
    while (l = gets(sep))
      yield l
    end
    self
  end
  alias_method :each_line, :each

  # BLOCKING eof? — real-pipe semantics: waits until a byte arrives
  # (false; byte pushed back) or every write end closes (true). The
  # parallel gem's forked worker loops `until read.eof?` between
  # Marshal frames.
  def eof?
    raise IOError, "closed stream" if @closed
    return false unless @pb.empty?
    b =
      if ::Thread.__coop_active?
        __coop_read_exact(1)
      else
        __rubyrs_fd_read(@fd, 1)
      end
    return true if b.nil?
    @pb << b
    false
  end
  alias_method :eof, :eof?

  def rewind
    raise Errno::ESPIPE, "Illegal seek"
  end

  def binmode; self; end
  def set_encoding(*_a); self; end

  def close
    return nil if @closed
    @closed = true
    __rubyrs_fd_close(@fd)
    nil
  end

  def closed?
    @closed
  end
end

class RubyrsFdWriter
  def initialize(fd)
    @fd = fd
    @closed = false
  end

  def fileno
    @fd
  end

  def write(*args)
    raise IOError, "closed stream" if @closed
    total = 0
    if ::Thread.__coop_active?
      # A write against a FULL pipe buffer parks the calling green
      # thread on (fd, :w) instead of blocking the VM in write(2);
      # EPIPE surfaces from the step exactly like the blocking path.
      # `while` (not `args.each`): a fiber cannot suspend across a
      # NATIVE iterator frame (vm/iter.rs truncation) — every loop
      # around a park point must be pure Ruby.
      ai = 0
      while ai < args.length
        s = args[ai].to_s
        off = 0
        size = s.bytesize
        while off < size
          r = __rubyrs_fd_write_step(@fd, s, off)
          if r == false
            ::Thread.__coop_wait_fd(@fd, :w)
          else
            off += r
          end
        end
        total += size
        ai += 1
      end
    else
      args.each do |a|
        total += __rubyrs_fd_write(@fd, a.to_s)
      end
    end
    total
  end

  # A pipe write only blocks against a FULL pipe buffer; this "non-
  # blocking" veneer performs the plain write (single-threaded rubyrs
  # can't be mid-drain elsewhere) and keeps CRuby's EPIPE contract:
  # `exception: false` suppresses EAGAIN/EWOULDBLOCK, never EPIPE.
  def write_nonblock(s, exception: true)
    raise IOError, "closed stream" if @closed
    __rubyrs_fd_write(@fd, s.to_s)
  end

  def <<(s)
    write(s)
    self
  end

  # `while` loops (not `each`): `write` can park a green thread, and
  # a fiber cannot suspend across a native iterator frame.
  def puts(*args)
    if args.empty?
      write("\n")
    else
      i = 0
      while i < args.length
        s = args[i].to_s
        write(s)
        write("\n") unless s.end_with?("\n")
        i += 1
      end
    end
    nil
  end

  def print(*args)
    i = 0
    while i < args.length
      write(args[i].to_s)
      i += 1
    end
    nil
  end

  def flush; self; end
  def sync; true; end
  def sync=(_v); _v; end
  def binmode; self; end
  def set_encoding(*_a); self; end

  def close
    return nil if @closed
    @closed = true
    __rubyrs_fd_close(@fd)
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
