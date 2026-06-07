# IO#write returns the number of BYTES written, not characters — they
# differ for multibyte content.
require "fileutils"
base = "/tmp/rubyrs_diff_writeret"

results = []
File.open(base, "w") do |f|
  results << f.write("café")     # 5 bytes
  results << f.write("αβ")       # 4 bytes
  results << f.write("ascii")    # 5 bytes
  results << f.write("a", "βγ")  # 1 + 4 = 5 bytes (multiple args)
end
p results
p results.sum
p File.read(base).bytesize

FileUtils.rm_f(base)
