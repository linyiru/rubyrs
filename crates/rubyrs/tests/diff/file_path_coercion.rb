# File class methods coerce a path-like ARGUMENT to a String via `to_path`
# (preferred) or `to_str`, like CRuby — so a Pathname / Tempfile / any
# object responding to those works. rack's spec_multipart calls
# `File.extname(env["rack.tempfiles"][0])` on a Tempfile object.

class PathLike
  def initialize(p) = (@p = p)
  def to_path = @p
end

class StrLike
  def initialize(p) = (@p = p)
  def to_str = @p
end

pl = PathLike.new("/tmp/dir/archive.tar.gz")
p File.extname(pl)            # ".gz"
p File.basename(pl)           # "archive.tar.gz"
p File.dirname(pl)            # "/tmp/dir"

sl = StrLike.new("a/b/c.txt")
p File.extname(sl)            # ".txt"
p File.basename(sl)           # "c.txt"

# plain strings still work
p File.extname("x.rb")        # ".rb"

# an object with NEITHER conversion raises TypeError (CRuby parity)
class Opaque; end
begin
  File.extname(Opaque.new)
rescue TypeError => e
  puts "TypeError raised"
end
