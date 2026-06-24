# IO.read / IO.write — File < IO in CRuby, so the core I/O class methods
# live on IO and File inherits them. Sinatra's `enable :inline_templates`
# does `IO.read(file)`. File-specific class methods (exist?/dirname) stay
# NoMethodError on IO, matching CRuby.
path = "/tmp/_io_read_fixture.txt"
IO.write(path, "line1\nline2\n")
p IO.read(path)
p IO.read(path, 5)
p File.read(path) == IO.read(path)
begin; IO.exist?(path); rescue NoMethodError; p :no_io_exist; end
File.delete(path)
