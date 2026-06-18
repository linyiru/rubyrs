# File.rename(old, new) — atomic rename, returns 0.
require "tmpdir"
Dir.mktmpdir do |d|
  a = File.join(d, "a.txt"); b = File.join(d, "b.txt")
  File.write(a, "hello")
  p File.rename(a, b)
  p File.exist?(a)
  p File.exist?(b)
  p File.read(b)
end
