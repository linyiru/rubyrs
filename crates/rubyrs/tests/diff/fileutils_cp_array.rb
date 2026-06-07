# FileUtils.cp with an Array of sources copies EVERY source into the
# destination directory. Regression: the old code took only the first
# source (`.next()`), silently dropping the rest — data loss.
require "fileutils"
base = "/tmp/rubyrs_diff_cp_array"
FileUtils.rm_rf(base)
FileUtils.mkdir_p("#{base}/dest")
File.write("#{base}/a.txt", "A")
File.write("#{base}/b.txt", "B")
File.write("#{base}/c.txt", "C")

FileUtils.cp(["#{base}/a.txt", "#{base}/b.txt", "#{base}/c.txt"], "#{base}/dest")
p Dir.entries("#{base}/dest").reject { |e| e == "." || e == ".." }.sort
p File.read("#{base}/dest/b.txt")
p File.read("#{base}/dest/c.txt")

# Single source into an existing dir → dest/basename.
FileUtils.cp("#{base}/a.txt", "#{base}/dest")
p File.exist?("#{base}/dest/a.txt")
# Single source to a non-dir dest → verbatim.
FileUtils.cp("#{base}/a.txt", "#{base}/renamed.txt")
p File.read("#{base}/renamed.txt")

FileUtils.rm_rf(base)
p File.exist?(base)
