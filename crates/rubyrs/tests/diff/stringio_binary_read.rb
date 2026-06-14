# StringIO#read(n) (with an explicit length) always returns ASCII-8BIT,
# regardless of the source string's encoding — CRuby's IO#read contract
# (a fixed byte count may split a multibyte char). A full read (no
# length) keeps the source encoding. rack's multipart parser reads a
# UTF-8-tagged-but-binary body in bufsize chunks and scans them byte-wise;
# a UTF-8 tag on the chunks would make subsequent ops lossy.
require 'stringio'

io = StringIO.new("héllo wörld")     # UTF-8 source (multibyte)
chunk = io.read(5)
puts "read(5) enc=#{chunk.encoding}"             # ASCII-8BIT

io2 = StringIO.new("plain")
puts "read(all) enc=#{io2.read.encoding}"        # UTF-8 (source enc)

io3 = StringIO.new("xyz")
buf = String.new
io3.read(2, buf)
puts "read(2,buf) enc=#{buf.encoding}"           # ASCII-8BIT

# byte-faithful chunked read over a binary payload: the running total
# matches the source bytesize (no lossy expansion / re-read).
src = ("\x89PNG\r\n\xFF\xC3data" * 500).b
io4 = StringIO.new(src)
total = 0
while (c = io4.read(2048, String.new))
  break if c.bytesize == 0
  total += c.bytesize
end
puts "chunked total=#{total} (expected #{src.bytesize})"
