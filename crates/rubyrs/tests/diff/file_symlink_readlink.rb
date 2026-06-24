# File.symlink? (lstat, no follow) and File.readlink (one-level target).
# Companions to File.symlink; FileUtils/dev-reloaders probe symlink?
# before readlink. symlink? is false for absent / non-link paths;
# readlink raises EINVAL on a non-link, ENOENT on a missing path.
require "tmpdir"
require "fileutils"
dir = Dir.mktmpdir
begin
  real = File.join(dir, "real.txt")
  File.write(real, "hi")
  link = File.join(dir, "link.txt")
  File.symlink(real, link)

  p File.symlink?(link)                  # true
  p File.symlink?(real)                  # false (regular file)
  p File.symlink?(File.join(dir, "no"))  # false (absent)
  p File.readlink(link) == real          # true
  begin; File.readlink(real); rescue SystemCallError => e; p e.class; end
  begin; File.readlink(File.join(dir, "no")); rescue SystemCallError => e; p e.class; end
ensure
  FileUtils.rm_rf(dir)
end
