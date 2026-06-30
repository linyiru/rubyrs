# Vendored `zlib` — Zlib::Deflate/Inflate + GzipWriter/GzipReader over
# the native flate2-backed host primitives (__zlib_deflate/_inflate/
# _gzip/_gunzip; see vm/kernel.rs + zlib_native.rs). rack's Deflater
# emits gzip responses via GzipWriter and its spec decompresses with
# GzipReader / Inflate.new(-MAX_WBITS); Static serves `.gz` files read
# back through GzipReader.wrap.
#
# Model: the streaming Deflate/Inflate/Gzip* objects BUFFER their
# input and run a single (de)compression at finish/close. A
# concatenated `deflate(x) + finish` stream decompresses identically
# to CRuby's incremental output, and the gem specs verify the
# round-tripped CONTENT (not the exact compressed bytes), so the
# buffered model is observationally equivalent for that surface.
module Zlib
  MAX_WBITS = 15

  NO_COMPRESSION      = 0
  BEST_SPEED          = 1
  BEST_COMPRESSION    = 9
  DEFAULT_COMPRESSION = -1

  # Flush flags (accepted + ignored by the buffered model).
  NO_FLUSH   = 0
  SYNC_FLUSH = 2
  FULL_FLUSH = 3
  FINISH     = 4

  DEFAULT_STRATEGY = 0

  class Error < StandardError; end
  class StreamEnd < Error; end
  class NeedDict < Error; end
  class DataError < Error; end
  class StreamError < Error; end
  class MemError < Error; end
  class BufError < Error; end
  class VersionError < Error; end

  module_function

  def deflate(str, level = DEFAULT_COMPRESSION)
    __zlib_deflate_zlib(str.to_s.b, level)
  end

  def inflate(str)
    __zlib_inflate_zlib(str.to_s.b)
  end

  def gzip(str, level: DEFAULT_COMPRESSION, strategy: DEFAULT_STRATEGY)
    __zlib_gzip(str.to_s.b, level || DEFAULT_COMPRESSION, 0)
  end

  def gunzip(str)
    __zlib_gunzip(str.to_s.b)[0]
  end

  # `Zlib.crc32(string = "", crc = 0)` — IEEE CRC-32, optionally
  # continuing a prior checksum. RuboCop's result-cache hashes source
  # files with it.
  def crc32(string = "", crc = 0)
    __zlib_crc32(string.to_s.b, crc)
  end

  # Raw / zlib DEFLATE compressor. `window_bits < 0` selects the raw
  # (headerless) stream rack's `deflate` encoding uses.
  class Deflate
    def self.deflate(str, level = DEFAULT_COMPRESSION)
      Zlib.deflate(str, level)
    end

    def initialize(level = DEFAULT_COMPRESSION, window_bits = MAX_WBITS, *_rest)
      @level = level || DEFAULT_COMPRESSION
      @raw = window_bits < 0
      @buf = +"".b
    end

    def deflate(str, _flush = NO_FLUSH)
      @buf << str.to_s.b if str
      "".b
    end

    def <<(str)
      deflate(str)
      self
    end

    def finish
      out = @raw ? __zlib_deflate(@buf, @level) : __zlib_deflate_zlib(@buf, @level)
      @buf = +"".b
      out
    end

    def flush(_flush = SYNC_FLUSH)
      "".b
    end

    def close
      "".b
    end
  end

  # INFLATE decompressor. `window_bits`: < 0 raw, 8..15 zlib, > 15
  # (16 / 32 + MAX_WBITS) auto-detects gzip vs zlib.
  class Inflate
    def self.inflate(str)
      Zlib.inflate(str)
    end

    def initialize(window_bits = MAX_WBITS, *_rest)
      @wbits = window_bits
      # Stateful native decompressor (opened lazily on first inflate)
      # so a stream fed in CHUNKS — e.g. rack's Deflater consumer
      # reading one sync-flushed gzip part at a time — decodes
      # incrementally, returning whatever is decodable so far. A
      # single `inflate(whole_stream)` still returns the full result
      # (one push decodes everything).
      @stream = nil
    end

    def inflate(str)
      @stream ||= __zlib_inflate_stream_new(@wbits)
      __zlib_inflate_stream_push(@stream, str.to_s.b)
    end

    def <<(str)
      inflate(str)
      self
    end

    # No buffered tail in this model — the incremental pushes already
    # returned all decodable output. Free the native handle.
    def finish
      __zlib_stream_free(@stream) if @stream
      @stream = nil
      "".b
    end

    def close
      __zlib_stream_free(@stream) if @stream
      @stream = nil
      "".b
    end

    def reset
      __zlib_stream_free(@stream) if @stream
      @stream = nil
      self
    end
  end

  class GzipFile
    class Error < Zlib::Error; end
    class NoFooter < Error; end
    class CRCError < Error; end
    class LengthError < Error; end
  end

  # gzip COMPRESSOR writing to an IO-like sink (responds to #write).
  # Buffers writes and emits one gzip member (with the header mtime)
  # on close. rack's Deflater wraps its response writer with this.
  class GzipWriter < GzipFile
    def self.wrap(io, *args)
      gz = new(io, *args)
      begin
        yield gz
      ensure
        gz.close
      end
    end

    def initialize(io, level = DEFAULT_COMPRESSION, _strategy = DEFAULT_STRATEGY, **_opts)
      @io = io
      @level = level || DEFAULT_COMPRESSION
      @mtime = nil
      @closed = false
      @finished = false
      # Lazily opened on first write/flush/finish so the header mtime
      # reflects a `gzip.mtime = …` set after construction (CRuby
      # contract; rack's Deflater sets it before the first write).
      @stream = nil
    end

    attr_accessor :mtime

    # Streaming gzip: each `write` feeds the native compressor and
    # forwards whatever compressed bytes it produces, and `flush`
    # emits a Z_SYNC_FLUSH boundary the reader can decode immediately
    # (rack's Deflater calls `write` then `flush` per chunk with
    # `:sync` true). Buffering everything until `finish` would defeat
    # streaming — a client that stops reading early would still see
    # the whole body compressed (spec_deflater "client aborts").
    def open_stream
      return @stream if @stream
      mt = if @mtime.nil?
             Time.now.to_i
           elsif @mtime.respond_to?(:to_i)
             @mtime.to_i
           else
             @mtime
           end
      @stream = __zlib_gz_deflate_new(@level, mt)
    end
    private :open_stream

    def write(data)
      s = data.to_s
      open_stream
      out = __zlib_gz_deflate_push(@stream, s.b, 0) # NO_FLUSH
      @io.write(out) unless out.empty?
      s.bytesize
    end

    def <<(data)
      write(data)
      self
    end

    def print(*args)
      args.each { |a| write(a) }
      nil
    end

    def printf(fmt, *args)
      write(sprintf(fmt, *args))
      nil
    end

    def flush(_flush = SYNC_FLUSH)
      open_stream
      out = __zlib_gz_deflate_push(@stream, "".b, 1) # SYNC_FLUSH
      @io.write(out) unless out.empty?
      self
    end

    # `finish` emits the final deflate block + gzip trailer (CRC +
    # ISIZE) but, unlike `close`, leaves the underlying IO OPEN (CRuby
    # contract). rack's Deflater calls `gzip.finish` in an ensure, and
    # the spec asserts the wrapped app body is NOT closed — so finish
    # must not cascade a close.
    def finish
      unless @finished
        open_stream
        out = __zlib_gz_deflate_push(@stream, "".b, 2) # FINISH (frees handle)
        @io.write(out) unless out.empty?
        @stream = nil
        @finished = true
      end
      @io
    end

    def close
      return @io if @closed
      finish
      @closed = true
      @io.close if @io.respond_to?(:close)
      @io
    end
  end

  # gzip DECOMPRESSOR reading a gzip stream from an IO-like source
  # (responds to #read) or a String. rack's Static reads `.gz` files
  # via `GzipReader.wrap(StringIO.new(body), &:read)`.
  class GzipReader < GzipFile
    def self.wrap(io, *args)
      gz = new(io, *args)
      begin
        yield gz
      ensure
        gz.close
      end
    end

    def self.open(path, *args)
      gz = new(File.open(path, "rb"), *args)
      return gz unless block_given?
      begin
        yield gz
      ensure
        gz.close
      end
    end

    def initialize(io, **_opts)
      content = io.respond_to?(:read) ? io.read : io.to_s
      @data, @mtime_i = __zlib_gunzip(content.to_s.b)
      @pos = 0
      @closed = false
    end

    def mtime
      Time.at(@mtime_i)
    end

    def read(length = nil)
      total = @data.bytesize
      if length.nil?
        r = @data.byteslice(@pos, total - @pos) || "".b
        @pos = total
        r
      else
        return nil if @pos >= total && length > 0
        r = @data.byteslice(@pos, length) || "".b
        @pos += r.bytesize
        r
      end
    end

    def gets(sep = "\n")
      return nil if @pos >= @data.bytesize
      idx = @data.byteindex(sep, @pos)
      if idx
        line = @data.byteslice(@pos, idx + sep.bytesize - @pos)
        @pos = idx + sep.bytesize
      else
        line = @data.byteslice(@pos, @data.bytesize - @pos)
        @pos = @data.bytesize
      end
      line
    end

    def each_line(sep = "\n")
      while (line = gets(sep))
        yield line
      end
      self
    end
    alias each each_line

    def readlines(sep = "\n")
      out = []
      while (line = gets(sep))
        out << line
      end
      out
    end

    def eof?
      @pos >= @data.bytesize
    end

    def rewind
      @pos = 0
    end

    def close
      @closed = true
      nil
    end

    def closed?
      @closed
    end
  end
end
