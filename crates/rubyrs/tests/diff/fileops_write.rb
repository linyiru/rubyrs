# Filesystem write surface: FileUtils.mkdir_p / cp / rm_rf, Dir.chdir
# (block form, cwd restored), File.write (+ opts) / File.read (+ length).
require "fileutils"
base = "/tmp/rubyrs_diff_fileops"
FileUtils.rm_rf(base)
FileUtils.mkdir_p("#{base}/sub")
File.write("#{base}/a.txt", "hello")
File.write("#{base}/sub/b.txt", "world", mode: "w")
FileUtils.cp("#{base}/a.txt", "#{base}/sub/a_copy.txt")
p File.read("#{base}/a.txt")
p File.read("#{base}/a.txt", 3)
p File.read("#{base}/sub/a_copy.txt")
before = Dir.pwd
names = Dir.chdir(base) { Dir["**/*.txt"].sort }
p names
p (Dir.pwd == before)
FileUtils.touch("#{base}/t")
p File.exist?("#{base}/t")
FileUtils.rm_rf(base)
p File.exist?(base)
