# Zlib veneer over the flate2 host primitives (stdlib). Compressed
# bytes aren't byte-equal to CRuby (different encoder), so we verify
# DECOMPRESSED content + round-trips, which ARE identical on both.
# rack's Deflater (gzip responses) and Static (serve .gz) ride this.
require "zlib"
require "stringio"

def t(l); r = begin; yield.inspect; rescue => e; "#{e.class}: #{e.message[0, 60]}"; end; puts "#{l}: #{r}"; end

s = "the quick brown fox jumps over the lazy dog. " * 4

t("gzip rt")    { Zlib.gunzip(Zlib.gzip(s)) == s }
t("deflate rt") { Zlib.inflate(Zlib.deflate(s)) == s }
t("empty gz")   { Zlib.gunzip(Zlib.gzip("")) == "" }
t("gz binary")  { Zlib.gzip(s).encoding.to_s }            # ASCII-8BIT

# Raw deflate (-MAX_WBITS) — rack's 'deflate' content-encoding shape.
t("raw rt") do
  d = Zlib::Deflate.new(6, -Zlib::MAX_WBITS)
  raw = d.deflate(s) << d.finish
  inf = Zlib::Inflate.new(-Zlib::MAX_WBITS)
  (inf.inflate(raw) << inf.finish) == s
end

# Auto-detect inflater (32 + MAX_WBITS) over a gzip stream.
t("auto gz")   { Zlib::Inflate.new(32 + Zlib::MAX_WBITS).inflate(Zlib.gzip(s)) == s }

# GzipWriter -> GzipReader through a StringIO, with a fixed header
# mtime (Time.now would be non-deterministic). finish leaves the IO
# open; close cascades.
t("gzipwriter") do
  io = StringIO.new
  w = Zlib::GzipWriter.new(io)
  w.mtime = 1_000_000
  w << "ab"
  w.write("cd")
  w.finish
  r = Zlib::GzipReader.new(StringIO.new(io.string))
  [r.read, r.mtime.to_i]
end

t("gzipreader wrap") do
  io = StringIO.new
  Zlib::GzipWriter.wrap(io) { |w| w.write("hello gzip") }
  Zlib::GzipReader.wrap(StringIO.new(io.string), &:read)
end

t("MAX_WBITS")  { Zlib::MAX_WBITS }
t("DataError")  { Zlib::DataError.ancestors.include?(StandardError) }
t("bad inflate"){ begin; Zlib.inflate("not zlib data!!"); rescue Zlib::DataError => e; e.class.name; end }
