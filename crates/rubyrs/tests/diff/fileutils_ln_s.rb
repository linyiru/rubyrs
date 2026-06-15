# FileUtils.ln_s(src, dest) creates a symbolic link (twin of
# File.symlink, but with cp-style dest-is-a-directory joining).
# Returns 0. zeitwerk's eager-load / ruby-compatibility tests link
# fixture directories this way.
require "fileutils"
base = "/tmp/rubyrs_diff_ln_s"
FileUtils.rm_rf(base)
FileUtils.mkdir_p("#{base}/real")
File.write("#{base}/real/x.rb", "X")

# Link a directory; read through it.
p FileUtils.ln_s("#{base}/real", "#{base}/linkdir")
p File.read("#{base}/linkdir/x.rb")

# Link a single file to a non-dir dest (verbatim).
p FileUtils.ln_s("#{base}/real/x.rb", "#{base}/x_link.rb")
p File.read("#{base}/x_link.rb")

# Link a file INTO an existing directory → dir/basename.
FileUtils.mkdir_p("#{base}/into")
FileUtils.ln_s("#{base}/real/x.rb", "#{base}/into")
p File.read("#{base}/into/x.rb")

# ln_sf forces over an existing link.
FileUtils.ln_sf("#{base}/real/x.rb", "#{base}/x_link.rb")
p File.read("#{base}/x_link.rb")

FileUtils.rm_rf(base)
p File.exist?(base)
