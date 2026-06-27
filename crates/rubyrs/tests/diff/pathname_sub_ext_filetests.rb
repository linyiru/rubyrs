# Pathname#sub_ext + File-status delegators (exist?/file?/directory?/read).
# Bridgetown's liquid partial renderer probes `path.sub_ext(".liquid")`,
# `path_variants.find(&:exist?)`, then `.read`s the winner.
require "pathname"
p Pathname.new("foo/bar.md").sub_ext(".html").to_s   # "foo/bar.html"
p Pathname.new("noext").sub_ext(".x").to_s            # "noext.x"
p Pathname.new("a.b.c").sub_ext("").to_s              # "a.b"

dir = "/tmp/rubyrs_pn_fixture"
require "fileutils"
FileUtils.mkdir_p(dir)
file = "#{dir}/hello.txt"
File.write(file, "content here")
p Pathname.new(file).exist?       # true
p Pathname.new(file).file?        # true
p Pathname.new(dir).directory?    # true
p Pathname.new(file).read         # "content here"
p Pathname.new("#{dir}/nope").exist?  # false
FileUtils.rm_rf(dir)
