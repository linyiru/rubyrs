# IO::SEEK_* whence constants (CRuby values) and File#seek honoring
# them. mini_mime's PReadFile#pread does
# `@file.seek(offset, IO::SEEK_SET); @file.read(size)`.
p IO::SEEK_SET   # 0
p IO::SEEK_CUR   # 1
p IO::SEEK_END   # 2

# Lexical fallback: a nested namespace referencing IO::SEEK_SET
# resolves to the toplevel ::IO constant.
module Outer
  module Inner
    WHENCE = IO::SEEK_SET
  end
end
p Outer::Inner::WHENCE   # 0

path = "/tmp/rubyrs_io_seek_#{Process.pid}.bin"
begin
  File.open(path, "wb") { |f| f.write("0123456789") }
  File.open(path, "rb") do |f|
    f.seek(3, IO::SEEK_SET)
    p f.read(2)          # "34"
    f.seek(0, IO::SEEK_SET)
    p f.read(1)          # "0"
  end
ensure
  File.delete(path) if File.exist?(path)
end
