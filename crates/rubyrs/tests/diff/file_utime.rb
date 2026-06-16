# `File.utime(atime, mtime, *paths)` — sets times, returns file count.
# Accepts Integer epoch and Time args.
require "tmpdir"
Dir.mktmpdir do |d|
  a = File.join(d, "a"); b = File.join(d, "b")
  File.write(a, "x"); File.write(b, "y")
  p File.utime(1_700_000_000, 1_700_000_001, a, b)   # count = 2
  p File.mtime(a).to_i
  p File.mtime(b).to_i
  t = Time.at(1_600_000_000)
  File.utime(t, t, a)
  p File.mtime(a).to_i
end
