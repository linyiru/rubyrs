# A qualified constant write `A::B = v` must RESOLVE its owner `A` first — like
# CRuby, which fires A's autoload to get a module to assign B on. rubyrs used to
# insert the flat `A::B` key without ever touching A, so an autoloaded owner was
# never triggered (zeitwerk autovivify-on-direct-require relies on this).
require "fileutils"
dir = File.join(__dir__, "qwo_tmp_xz")
FileUtils.mkdir_p(dir)
File.write(File.join(dir, "owns.rb"), "module OwNsXz; LOADED = true; end")

Object.autoload(:OwNsXz, File.join(dir, "owns.rb"))
OwNsXz::Extra = 42      # writing Extra on OwNsXz must fire OwNsXz's autoload first

p OwNsXz::LOADED        # true — the owner autoload fired
p OwNsXz::Extra         # 42
FileUtils.rm_rf(dir)
