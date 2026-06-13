# Tier 3 Tempfile — a buffered handle over a real file in the temp
# directory (filesystem access rides the existing
# `allow_filesystem_io` capability; with it off, flush/read raise
# the same IOError every File call does). Names combine pid + a
# process-local sequence + the basename — collision-free within a
# run and deterministic in ORDER (the path embeds the pid, which is
# host state the consumer explicitly asked for by using Tempfile).
#
# Covers the minitest surface: assertions#diff writes a pair and
# shells out (`Tempfile.open { |f| f.puts ...; f.flush }` + #path),
# and capture_subprocess_io reopens stdio onto one (#16). Out of
# scope: encoding modes, real IO inheritance to subprocesses beyond
# path-based access, auto-finalizer unlink (close!/unlink are
# explicit — minitest always calls them).
class Tempfile
  @@seq = 0

  def self.open(basename = "tmp")
    f = new(basename)
    return f unless block_given?
    begin
      yield f
    ensure
      f.close!
    end
  end

  def initialize(basename = "tmp")
    @@seq += 1
    dir = ENV["TMPDIR"] || "/tmp"
    dir = dir.chomp("/")
    @path = "#{dir}/rubyrs-#{Process.pid}-#{@@seq}-#{basename}"
    @buf = +""
    @flushed = false
    @closed = false
    @pos = 0
  end

  attr_reader :path

  def write(s)
    s = s.to_s
    @buf << s
    # CRuby's Tempfile is a real IO: writes advance the SAME
    # cursor reads use, so write-then-gets sees EOF until a
    # rewind. Mirror that (rack's RewindableInput depends on the
    # rewind-before-read discipline).
    @pos = @buf.bytesize
    @flushed = false
    s.length
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

  def flush
    File.write(@path, @buf)
    @flushed = true
    self
  end

  def rewind
    @pos = 0
    0
  end

  # rack RewindableInput surface: it buffers a request body into a
  # Tempfile, locks it down, and serves IO reads off it. The
  # permission/encoding calls are accepted no-ops on this buffered
  # handle (the buffer is process-private; reads are byte-faithful
  # via File.binread regardless), and the read side is BYTE-based
  # with the `(length, outbuf)` IO contract.
  def chmod(_mode)
    0
  end

  def set_encoding(_enc, *_rest)
    self
  end

  def binmode
    self
  end

  def binmode?
    true
  end

  def fsync
    flush
    0
  end

  def read(length = nil, outbuf = nil)
    flush unless @flushed
    content = File.binread(@path)
    total = content.bytesize
    result =
      if length.nil?
        out = content.byteslice(@pos, total - @pos) || ""
        @pos = total
        out
      else
        chunk = content.byteslice(@pos, length) || ""
        @pos += chunk.bytesize
        chunk.bytesize == 0 && length > 0 ? nil : chunk
      end
    if outbuf
      outbuf.replace(result || "")
      result.nil? ? nil : outbuf
    else
      result
    end
  end

  def gets(sep = "\n")
    flush unless @flushed
    content = File.binread(@path)
    total = content.bytesize
    return nil if @pos >= total
    idx = content.byteindex(sep, @pos)
    if idx
      line = content.byteslice(@pos, idx + sep.bytesize - @pos)
      @pos = idx + sep.bytesize
    else
      line = content.byteslice(@pos, total - @pos) || ""
      @pos = total
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
    flush unless @flushed
    @pos >= File.binread(@path).bytesize
  end

  def size
    @buf.bytesize
  end
  alias_method :length, :size

  def close
    flush unless @flushed
    @closed = true
    nil
  end

  def closed?
    @closed
  end

  def unlink
    File.delete(@path) if File.exist?(@path)
    nil
  rescue StandardError
    nil
  end
  alias_method :delete, :unlink

  def close!
    close
    unlink
  end
end
