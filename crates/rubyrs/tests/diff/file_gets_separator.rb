# File#gets / #readlines include the FULL record separator in each line.
# Regression: a multi-character separator ("XX") was truncated to its
# first character ("X").
require "fileutils"
base = "/tmp/rubyrs_diff_gets"

File.write(base, "aXXbXXcXXd")
File.open(base) do |f|
  p f.gets("XX")   # "aXX"
  p f.gets("XX")   # "bXX"
  p f.gets("XX")   # "cXX"
  p f.gets("XX")   # "d"
  p f.gets("XX")   # nil
end

# Single-character separator is unaffected.
File.write(base, "a\nbb\nccc")
File.open(base) do |f|
  p f.gets         # "a\n"
  p f.gets         # "bb\n"
  p f.gets         # "ccc"
end

# readlines with a multi-char separator.
File.write(base, "1::2::3")
p File.open(base) { |f| f.readlines("::") }   # ["1::", "2::", "3"]

FileUtils.rm_f(base)
