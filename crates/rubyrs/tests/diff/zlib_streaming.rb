# Stateful streaming gzip: Zlib::GzipWriter writes incrementally with
# per-chunk flush (rack Deflater's `:sync` path), and Zlib::Inflate
# decodes a stream fed in pieces. Only DECODED content is printed —
# the exact compressed bytes are not byte-canonical across zlib
# implementations, but the round-trip content is.

require 'zlib'
require 'stringio'

# write in chunks with an intermediate flush, then finish
sink = StringIO.new
gz = Zlib::GzipWriter.new(sink)
gz.write("Hello, ")
gz.flush
gz.write("world!")
gz.finish
compressed = sink.string

# one-shot decoders agree
p Zlib.gunzip(compressed)
p Zlib::Inflate.new(32 + Zlib::MAX_WBITS).inflate(compressed)

# incremental decode: feed the complete stream in two byte-halves; the
# concatenation of the per-push results is the full content regardless
# of where the split lands.
inf = Zlib::Inflate.new(32 + Zlib::MAX_WBITS)
cb = compressed.b
mid = cb.bytesize / 2
out = +"".b
out << inf.inflate(cb[0, mid])
out << inf.inflate(cb[mid..] || "".b)
p out

# larger multi-block body
big = ("abcd" * 4000) + ("z" * 1000)   # 17000 bytes
sink2 = StringIO.new
w = Zlib::GzipWriter.new(sink2)
w.write(big)
w.finish
round = Zlib.gunzip(sink2.string)
p(round == big)
p round.bytesize

# << operator + mtime accessor
sink3 = StringIO.new
g3 = Zlib::GzipWriter.new(sink3)
g3.mtime = 0
g3 << "a" << "b" << "c"
g3.close
p Zlib.gunzip(sink3.string)

# GzipReader still reads a complete member
gz_bytes = sink3.string
p Zlib::GzipReader.new(StringIO.new(gz_bytes)).read
