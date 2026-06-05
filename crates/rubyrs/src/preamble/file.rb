# Minimal File IO veneer — a Tier-3 `_io`-style pure-Ruby class
# layer on top of the Tier-1 `File.read` host primitive (ADR
# 0019: the `_io` battery is "a Ruby-class veneer on Tier-1
# host-fn primitives"). Reopens the `class File` defined in the
# inline PREAMBLE (which carries SEPARATOR / POSIX flag consts);
# loaded right after it so the class already exists.
#
# Surface covered (the load-time + read-side shape gems reach
# for):
#   * File.open(path, mode = ...) { |f| ... } block form —
#     yields a File instance, closes it on block exit (even on
#     exception), returns the block's value
#   * File.open(path) non-block form returning a File instance
#   * File.new(path) (String first arg) — same as open, no block
#   * File.new(fd, ...) (Integer first arg) — raises IOError
#     (rubyrs has no file-descriptor table; honest "feature
#     absent" rather than a fake fd)
#   * instance #read / #gets / #each_line / #each / #readlines /
#     #path / #close / #closed? / #eof? / #fileno
#
# Discovery: P3 Sinatra spike — logger 1.7
# `logger/log_device.rb:255` self-probes
#   File.open(__FILE__) do |f|
#     File.new(f.fileno, autoclose: false, path: "").path
#   rescue IOError
#     module PathAttr ... end
#   end
# at module-load time. `#fileno` raising IOError drives the
# probe's `rescue IOError` clause, so logger installs its
# PathAttr fallback — the same outcome CRuby reaches on a
# platform without the `path:` keyword. Without File.open the
# whole logger load (and thus Sinatra) died at
# `NoMethodError: undefined method 'open' for Class`.
#
# Divergences (documented, out of spike scope):
#   * read returns UTF-8-lossy chars, not raw bytes (inherits
#     the Tier-1 `File.read` behaviour — binary files get
#     U+FFFD substitution)
#   * no write/append/seek surface (read-only veneer)
#   * #fileno never returns a real descriptor — rubyrs is
#     sandboxed and has no fd table

class File
  def self.open(path, mode = "r", **_opts)
    # Reuse the capability-gated Tier-1 `File.read` primitive for
    # the actual disk reach. A read failure (missing file,
    # sandbox denial) surfaces as whatever `File.read` raises —
    # same error the caller would see from a bare `File.read`.
    buf = File.read(path)
    f = allocate
    f.__io_init(path.to_s, buf)
    if block_given?
      begin
        yield f
      ensure
        f.close
      end
    else
      f
    end
  end

  def self.new(fd_or_path, *args, **_opts)
    if fd_or_path.is_a?(Integer)
      # rubyrs has no file-descriptor table; reopening a raw fd
      # is genuinely unsupported. CRuby's File.for_fd surfaces an
      # unusable descriptor as IOError, so we mirror that — and
      # it's exactly the signal logger's load-time probe rescues.
      raise IOError, "rubyrs: cannot reopen file descriptor #{fd_or_path} (no fd table)"
    end
    open(fd_or_path, *args)
  end

  # --- instance surface (operates on the buffered content) ---

  # @!visibility private
  def __io_init(path, buf)
    @__io_path = path
    @__io_buf = buf
    @__io_pos = 0
    @__io_closed = false
    self
  end

  def read(length = nil)
    raise IOError, "closed stream" if @__io_closed
    rest = @__io_buf[@__io_pos..] || ""
    if length.nil?
      @__io_pos = @__io_buf.length
      rest
    else
      chunk = rest[0, length] || ""
      @__io_pos += chunk.length
      chunk.empty? ? nil : chunk
    end
  end

  def gets(sep = "\n")
    raise IOError, "closed stream" if @__io_closed
    return nil if @__io_pos >= @__io_buf.length
    idx = @__io_buf.index(sep, @__io_pos)
    if idx
      line = @__io_buf[@__io_pos..idx]
      @__io_pos = idx + sep.length
    else
      line = @__io_buf[@__io_pos..] || ""
      @__io_pos = @__io_buf.length
    end
    line
  end

  def each_line(sep = "\n")
    while (line = gets(sep))
      yield line
    end
    self
  end
  alias_method :each, :each_line

  def readlines(sep = "\n")
    lines = []
    while (line = gets(sep))
      lines << line
    end
    lines
  end

  def path
    @__io_path
  end

  def close
    @__io_closed = true
    nil
  end

  def closed?
    @__io_closed
  end

  def eof?
    @__io_pos >= @__io_buf.length
  end

  def fileno
    # rubyrs is sandboxed and has no file-descriptor table, so
    # there is no honest integer to return. CRuby's #fileno hands
    # back a real OS fd; we raise IOError instead. Drives logger's
    # load-time probe to its PathAttr fallback.
    raise IOError, "rubyrs: file descriptors are not available"
  end
end
