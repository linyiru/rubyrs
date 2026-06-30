# File.readlines / File.foreach class methods -- File < IO in CRuby,
# so these are the IO class methods File inherits. The veneer layers
# them over the buffered #readlines/#each_line instance surface.
# Round-trips a /tmp fixture and cleans up; output is the line arrays
# themselves (ASCII-only, so byte==char) for an exact live diff.
path = "/tmp/rubyrs_diff_readlines.txt"
File.write(path, "line1\nline2\nline3\n")

# Plain slurp.
p File.readlines(path)
# chomp: peels the trailing separator (default record-sep also "\r\n").
p File.readlines(path, chomp: true)
# Explicit separator + chomp combination.
crlf = "/tmp/rubyrs_diff_readlines_crlf.txt"
File.write(crlf, "a\r\nb\r\n")
p File.readlines(crlf, chomp: true)
sep = "/tmp/rubyrs_diff_readlines_sep.txt"
File.write(sep, "x;y;z;")
p File.readlines(sep, ";")
p File.readlines(sep, ";", chomp: true)

# foreach with a block yields each line and returns nil.
acc = []
ret = File.foreach(path) { |l| acc << l }
p acc
p ret.nil?

acc2 = []
File.foreach(path, chomp: true) { |l| acc2 << l }
p acc2

# Blockless foreach -> Enumerator.
p File.foreach(path).to_a
p File.foreach(path, chomp: true).map(&:upcase)

# IO.readlines / IO.foreach (File < IO in CRuby) share the surface.
p IO.readlines(path)
p IO.readlines(path, chomp: true)
io_acc = []
IO.foreach(path) { |l| io_acc << l }
p io_acc
p IO.foreach(path, chomp: true).to_a

[path, crlf, sep].each { |f| File.delete(f) }
