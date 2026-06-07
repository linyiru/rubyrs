# Reading from a write/append-only handle raises IOError (CRuby), rather
# than silently returning "". A "+"/read mode reads normally.
base = "/tmp/rubyrs_diff_whread"
File.write(base, "hello")

begin
  File.open(base, "w") { |f| f.read }
rescue IOError
  puts "w-read: IOError"
end

begin
  File.open(base, "a") { |f| f.gets }
rescue IOError
  puts "a-gets: IOError"
end

File.write(base, "world")
p File.open(base, "r") { |f| f.read }   # "world"
p File.open(base) { |f| f.read }        # default "r"

require "fileutils"
FileUtils.rm_f(base)
