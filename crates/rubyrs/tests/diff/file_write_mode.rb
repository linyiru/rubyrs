# File.write honours `mode: "a"` (append). Regression: the mode option
# was ignored and every write truncated, so an append silently
# overwrote the prior content.
require "fileutils"
base = "/tmp/rubyrs_diff_writemode"
FileUtils.rm_f(base)

File.write(base, "AAA")
File.write(base, "BBB", mode: "a")
p File.read(base)            # "AAABBB"

File.write(base, "CCC")      # default truncate
p File.read(base)            # "CCC"

File.write(base, "DD", mode: "ab")
p File.read(base)            # "CCCDD"

FileUtils.rm_f(base)
p File.exist?(base)
