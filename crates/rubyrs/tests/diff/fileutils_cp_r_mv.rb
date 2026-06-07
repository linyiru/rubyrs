# FileUtils.cp_r (recursive copy) and FileUtils.mv (rename / move into
# a directory). Regression: cp_r was documented but unimplemented
# (NoMethodError); mv was absent entirely.
require "fileutils"
base = "/tmp/rubyrs_diff_cprmv"
FileUtils.rm_rf(base)
FileUtils.mkdir_p("#{base}/src/sub")
File.write("#{base}/src/a.txt", "A")
File.write("#{base}/src/sub/c.txt", "C")
FileUtils.mkdir_p("#{base}/dest")

# cp_r: recursive copy of a tree to a new name.
FileUtils.cp_r("#{base}/src", "#{base}/dest/srccopy")
p File.read("#{base}/dest/srccopy/a.txt")
p File.read("#{base}/dest/srccopy/sub/c.txt")
p File.directory?("#{base}/dest/srccopy/sub")

# mv a file (rename in place).
File.write("#{base}/m1.txt", "M")
FileUtils.mv("#{base}/m1.txt", "#{base}/m2.txt")
p File.exist?("#{base}/m1.txt")
p File.read("#{base}/m2.txt")

# mv into an existing directory → dest/basename.
File.write("#{base}/m3.txt", "N")
FileUtils.mv("#{base}/m3.txt", "#{base}/dest")
p File.exist?("#{base}/dest/m3.txt")
p File.exist?("#{base}/m3.txt")

FileUtils.rm_rf(base)
p File.exist?(base)
