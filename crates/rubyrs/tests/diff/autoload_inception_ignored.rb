# CRuby ignores `autoload(:X, path)` when `path` is the file CURRENTLY being
# required — you can't autoload a file that is loading itself. So a file that
# does `autoload(:Self, __FILE__)` while it's mid-require leaves autoload?(:Self)
# == nil. (zeitwerk's "inception" edge.)
require "fileutils"
dir = File.join(__dir__, "incept_tmp_xz")
FileUtils.mkdir_p(dir)
File.write(File.join(dir, "incself.rb"), <<~RB)
  Object.autoload(:IncSelfXz, __FILE__)
  $inc_al = Object.autoload?(:IncSelfXz)
  IncSelfXz = 1
RB
$LOAD_PATH.unshift dir
require "incself"

p $inc_al            # nil — the in-flight file's autoload was ignored
p IncSelfXz          # 1
FileUtils.rm_rf(dir)
