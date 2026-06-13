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
    mode_s = mode.to_s
    # Split a mode-string encoding suffix ("r:bom|utf-8" →
    # flags "r", encoding "bom|utf-8"). Two fixes in one:
    # (1) flag detection must look at the FLAGS part only —
    # matching on the whole string misread "r:windows-31j" as a
    # WRITE mode (the 'w' in "windows"); (2) the encoding part
    # forwards to the Tier-1 `File.read` primitive, whose
    # "bom|utf-8" handling strips a leading UTF-8 BOM exactly
    # like CRuby's open-time BOM consumption. Other encodings
    # pass through and are ignored there (raw-bytes read), the
    # pre-existing behaviour.
    flags, enc = mode_s.split(":", 2)
    writing = flags.include?("w") || flags.include?("a")
    # Readable unless the mode is write/append-only ("w"/"a" without
    # "+"). A pure write handle must reject reads (CRuby IOError),
    # rather than silently returning "".
    reading = flags.include?("r") || flags.include?("+")
    read_now = lambda do
      if flags.include?("b")
        # Binary mode: byte-transparent read (BINARY tag) — the
        # text-mode primitive would U+FFFD-mangle invalid UTF-8
        # (addressable's `File.open(path, "rb") { Marshal.load
        # (f.read) }` over its pregenerated unicode.data).
        File.binread(path)
      elsif enc
        File.read(path, :encoding => enc)
      else
        File.read(path)
      end
    end
    if writing
      # Write/append mode: start from "" (truncate) or the existing
      # content (append). Buffered in memory; flushed to disk via the
      # Tier-1 `File.write` primitive on close. A read failure for the
      # append case (missing file) just starts empty.
      buf = flags.include?("a") ? (read_now.call rescue "") : ""
      f = allocate
      f.__io_init(path.to_s, buf, write: true, read: reading)
    else
      # Reuse the capability-gated Tier-1 `File.read` primitive for
      # the actual disk reach. A read failure (missing file, sandbox
      # denial) surfaces as whatever `File.read` raises.
      buf = read_now.call
      f = allocate
      f.__io_init(path.to_s, buf, read: reading)
    end
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

  # `File.mtime(path)` — last-modified Time. The epoch seconds come
  # from the native `__mtime_f` primitive (Float, sub-second); Time
  # is a Ruby-level class (preamble/time.rb), so the object is built
  # here via `Time.at`, mirroring how `Time.now` wraps its native
  # clock read. Rack::Files emits it as Last-Modified and compares it
  # against If-Modified-Since (`File.mtime(path).httpdate`).
  def self.mtime(path)
    Time.at(__mtime_f(path))
  end

  # `File.stat(path)` → File::Stat (FOLLOWS symlinks). The native
  # `__stat_raw` returns the metadata tuple; the Stat object exposes
  # the CRuby query surface Rack::Directory reaches for (mtime / size /
  # directory? / file? / readable?). mtime wraps via Time.at, mirroring
  # Time.now. A missing path (incl. a broken symlink's target) raises
  # Errno::ENOENT from the primitive — Rack turns that into a 404.
  def self.stat(path)
    Stat.new(__stat_raw(path))
  end

  # Buffered metadata snapshot. The native tuple order is fixed by
  # `File.__stat_raw` (vm/fileops.rs).
  class Stat
    def initialize(raw)
      @size, @mtime_f, @dir, @file, @mode, @symlink,
        @readable, @writable, @executable = raw
    end

    attr_reader :size, :mode

    def mtime
      Time.at(@mtime_f)
    end

    def directory?  ; @dir;        end
    def file?       ; @file;       end
    def symlink?    ; @symlink;    end
    def readable?   ; @readable;   end
    def writable?   ; @writable;   end
    def executable? ; @executable; end
    def zero?       ; @size == 0;  end
  end

  # --- instance surface (operates on the buffered content) ---

  # @!visibility private
  def __io_init(path, buf, write: false, read: true)
    @__io_path = path
    @__io_buf = buf
    @__io_pos = write ? buf.bytesize : 0
    @__io_closed = false
    @__io_write = write
    @__io_read = read
    @__io_dirty = false
    self
  end

  # --- write surface (accumulates in @__io_buf, flushed on close) ---

  def write(*strs)
    raise IOError, "not opened for writing" unless @__io_write
    raise IOError, "closed stream" if @__io_closed
    n = 0
    strs.each do |s|
      str = s.to_s
      @__io_buf << str
      # IO#write returns the number of BYTES written, not characters —
      # they differ for multibyte content.
      n += str.bytesize
    end
    @__io_dirty = true
    n
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
      return nil
    end
    args.each do |a|
      if a.is_a?(Array)
        a.empty? ? write("\n") : a.each { |e| puts(e) }
      else
        s = a.to_s
        s.end_with?("\n") ? write(s) : write(s, "\n")
      end
    end
    nil
  end

  def printf(fmt, *args)
    write(sprintf(fmt, *args))
    nil
  end

  # `read(N, outbuf)` second arg: the result is REPLACED into
  # outbuf and outbuf itself returned (same object — rack's
  # multipart parser reuses one buffer across chunk reads); the
  # EOF-nil path clears the buffer, matching CRuby.
  def read(length = nil, outbuf = nil)
    raise IOError, "closed stream" if @__io_closed
    raise IOError, "not opened for reading" unless @__io_read
    if length.nil? && @__io_pos == 0 && outbuf.nil?
      # Whole-buffer read: return a dup, NOT a slice — slicing a
      # BINARY/registry-tagged buffer through the char view
      # U+FFFD-mangles it (E1 boundary; addressable's
      # `File.open(path, "rb") { Marshal.load(f.read) }`).
      @__io_pos = @__io_buf.bytesize
      return @__io_buf.dup
    end
    # Byte-based cursor + byteslice throughout: CRuby's IO#read
    # length is BYTES, and byte slicing keeps the buffer's tag
    # without mangling non-UTF-8 content.
    total = @__io_buf.bytesize
    result =
      if length.nil?
        rest = @__io_buf.byteslice(@__io_pos, total - @__io_pos) || ""
        @__io_pos = total
        rest
      else
        chunk = @__io_buf.byteslice(@__io_pos, length) || ""
        @__io_pos += chunk.bytesize
        # read(N) at EOF is nil — but read(0) is always "".
        chunk.bytesize == 0 && length > 0 ? nil : chunk
      end
    if outbuf
      outbuf.replace(result || "")
      result.nil? ? nil : outbuf
    else
      result
    end
  end

  # Byte-cursor positioning over the buffered content. Rack::Files
  # serves Range requests with `file.seek(range.begin)` then chunked
  # `file.read(n)` (lib/rack/files.rb#each_range_part), so seek must
  # honour the BINARY/byte offsets the read cursor already uses.
  # `whence` ∈ {SEEK_SET=0, SEEK_CUR=1, SEEK_END=2}; seek returns 0.
  def seek(amount, whence = 0)
    base =
      case whence
      when 1 then @__io_pos           # IO::SEEK_CUR
      when 2 then @__io_buf.bytesize  # IO::SEEK_END
      else 0                          # IO::SEEK_SET
      end
    @__io_pos = base + amount
    0
  end

  def pos
    @__io_pos
  end
  alias_method :tell, :pos

  def pos=(n)
    @__io_pos = n
  end

  def rewind
    @__io_pos = 0
    0
  end

  def gets(sep = "\n")
    raise IOError, "closed stream" if @__io_closed
    raise IOError, "not opened for reading" unless @__io_read
    total = @__io_buf.bytesize
    return nil if @__io_pos >= total
    idx = @__io_buf.byteindex(sep, @__io_pos)
    if idx
      # Include the FULL separator in the returned line (CRuby).
      # Byte-level split: a registry/BINARY-tagged buffer must not
      # go through the lossy char view, and byteslice carries the
      # buffer's tag onto each line (CRuby line encodings).
      line = @__io_buf.byteslice(@__io_pos, idx + sep.bytesize - @__io_pos)
      @__io_pos = idx + sep.bytesize
    else
      line = @__io_buf.byteslice(@__io_pos, total - @__io_pos) || ""
      @__io_pos = total
    end
    line
  end

  # The handle's read encoding = the buffer's tag (the veneer
  # transcodes/tags at open, so the buffer IS the external view).
  # CRuby returns nil for write-only handles; the veneer keeps it
  # simple and reports the buffer tag whenever one exists.
  def external_encoding
    @__io_buf ? @__io_buf.encoding : Encoding::UTF_8
  end

  # Like #gets but raises EOFError at end of file instead of
  # returning nil (CRuby IO#readline). Discovery: P3 Jekyll spike —
  # `utils.rb#has_yaml_header?` does `File.open(f, "rb", &:readline)`
  # to sniff the first line for a `---` front-matter marker.
  def readline(sep = "\n")
    line = gets(sep)
    raise EOFError, "end of file reached" if line.nil?
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

  def flush
    if @__io_write && @__io_dirty
      File.write(@__io_path, @__io_buf)
      @__io_dirty = false
    end
    self
  end

  def close
    return nil if @__io_closed
    flush
    @__io_closed = true
    nil
  end

  def closed?
    @__io_closed
  end

  def eof?
    @__io_pos >= @__io_buf.bytesize
  end

  def fileno
    # rubyrs is sandboxed and has no file-descriptor table, so
    # there is no honest integer to return. CRuby's #fileno hands
    # back a real OS fd; we raise IOError instead. Drives logger's
    # load-time probe to its PathAttr fallback.
    raise IOError, "rubyrs: file descriptors are not available"
  end
end
