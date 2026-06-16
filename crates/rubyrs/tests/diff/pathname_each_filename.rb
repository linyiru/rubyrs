# `Pathname#each_filename` — yield each path component (drops leading
# `/`, collapses `//`); Enumerator without a block.
require "pathname"
p Pathname.new("/usr/bin/ruby").each_filename.to_a
out = []; Pathname.new("a/b//c").each_filename { |f| out << f }; p out
p Pathname.new("rel").each_filename.to_a
p Pathname.new("/").each_filename.to_a
