# Tier 3 pure-Ruby StringIO — buffer-only subset covering the
# common gem-helper pattern: `io = StringIO.new; io << "..."; io.string`.
# No real IO is involved (so no file descriptor, no truncation
# mode, no read/write mode flags); just a String + a position
# cursor.
#
# Methods that CRuby's stdlib/stringio.rb exposes but rubyrs
# Tier 3 doesn't carry here (binary modes, sysread, set_encoding,
# fdatasync, ...) reach for fs / encoding surface we don't model.
# Scripts that touch them get NoMethodError — the right
# "feature absent" surface.
#
# Gated behind the `stdlib` Cargo feature.

class StringIO
  def initialize(string = "")
    @str = string.dup
    @pos = 0
    @closed = false
  end

  # String#new-shaped factory aliases. CRuby has `StringIO.open`
  # which takes a block and auto-closes; the block form here
  # mirrors that for callers that copy the pattern.
  def self.open(string = "")
    io = new(string)
    return io unless block_given?
    begin
      yield io
    ensure
      io.close
    end
  end

  # ----- content + position primitives -----

  def string
    @str
  end

  def pos
    @pos
  end
  alias_method :tell, :pos

  def pos=(n)
    @pos = n
    n
  end

  def rewind
    @pos = 0
    0
  end

  def seek(amount, whence = 0)
    case whence
    when 0 then @pos = amount               # SEEK_SET (absolute)
    when 1 then @pos = @pos + amount        # SEEK_CUR (relative)
    when 2 then @pos = @str.length + amount # SEEK_END (from end)
    else
      raise ArgumentError, "invalid whence: #{whence}"
    end
    0
  end

  def size
    @str.length
  end
  alias_method :length, :size

  def eof?
    @pos >= @str.length
  end
  alias_method :eof, :eof?

  def closed?
    @closed
  end

  def close
    @closed = true
    nil
  end
  alias_method :close_read, :close
  alias_method :close_write, :close

  # ----- write side -----

  def write(*args)
    n = 0
    args.each do |a|
      s = a.to_s
      if @pos == @str.length
        @str << s
      else
        # In-place overwrite + extend. CRuby's exact byte-pad
        # semantics for sparse writes (NUL fill past EOF) aren't
        # modelled — gem helpers don't do sparse writes on a
        # fresh buffer.
        head = @str[0, @pos] || ""
        tail = @str[(@pos + s.length)..] || ""
        @str = head + s + tail
      end
      @pos += s.length
      n += s.length
    end
    n
  end

  def <<(obj)
    write(obj)
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
        s = a.to_s
        write(s)
        write("\n") unless s.end_with?("\n")
      end
    end
    nil
  end

  def printf(_fmt, *_args)
    raise NotImplementedError, "StringIO#printf not modelled in Tier 3 vendor"
  end

  # ----- read side -----

  def read(length = nil)
    return nil if length && length < 0
    if length.nil?
      result = @str[@pos..] || ""
      @pos = @str.length
      result
    else
      slice = @str[@pos, length] || ""
      @pos += slice.length
      # `read(N)` on EOF returns nil; on partial returns whatever's
      # left. Mirrors CRuby for the cases gem helpers care about.
      slice.empty? && length > 0 ? nil : slice
    end
  end

  def gets
    return nil if @pos >= @str.length
    # `String#index` two-arg form (with start offset) isn't on the
    # Tier 1 fast path; slice from `@pos` first and search the
    # resulting substring. One extra allocation per `gets`, but
    # the buffer-only StringIO shape isn't a hot path.
    remaining = @str[@pos..] || ""
    rel = remaining.index("\n")
    if rel
      line = remaining[0, rel + 1]
      @pos += rel + 1
      line
    else
      @pos = @str.length
      remaining
    end
  end

  def each_line(&block)
    return self unless block
    while (line = gets)
      block.call(line)
    end
    self
  end
  alias_method :each, :each_line

  def inspect
    "#<StringIO>"
  end
end
