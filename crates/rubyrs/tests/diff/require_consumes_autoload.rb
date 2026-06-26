# CRuby: `require`ing a file SATISFIES (consumes) any autoload registered for
# it — autoload? returns nil afterward (the autoload no longer fires). zeitwerk
# relies on this for autoload-on-self / require-up-the-chain edges.
require "fileutils"
dir = File.join(__dir__, "rc_tmp_xz")
FileUtils.mkdir_p(dir)
File.write(File.join(dir, "rcfoo.rb"), "RcFooXz = 1")

Object.autoload(:RcFooXz, File.join(dir, "rcfoo.rb"))
puts(Object.autoload?(:RcFooXz) ? "armed" : "nil")   # armed (registered)

$LOAD_PATH.unshift dir
require "rcfoo"

puts(Object.autoload?(:RcFooXz) ? "armed" : "nil")   # nil (consumed by require)
puts RcFooXz                                          # 1
FileUtils.rm_rf(dir)
