# `Dir.each_child(path) { |basename| ... }` — yields entries excluding
# "." and "..". Block and Enumerator (no-block) forms. zeitwerk's
# Loader::Helpers walks autoload directories this way.
base = "/tmp/rubyrs_each_child_fixture"
require "fileutils" rescue nil
system("rm", "-rf", base)
Dir.mkdir(base)
File.write(File.join(base, "b.rb"), "")
File.write(File.join(base, "a.rb"), "")
Dir.mkdir(File.join(base, "sub"))

# Block form — collect + sort (CRuby yields in FS order; rubyrs sorts).
collected = []
ret = Dir.each_child(base) { |name| collected << name }
p collected.sort
p ret

# No-block form returns an Enumerator.
p Dir.each_child(base).to_a.sort

system("rm", "-rf", base)
