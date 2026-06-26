# `remove_const` must clear the constant's recorded source location, so a later
# REDEFINITION reports the NEW location (CRuby). rubyrs's StoreConst is
# first-write-wins, so without clearing, a removed+redefined const kept its
# stale location — zeitwerk's reload + "already defined in <loc>" shadow log
# relies on the fresh location.
dir = File.join(__dir__, "rcsl_tmp_xz")
require "fileutils"
FileUtils.mkdir_p(dir)
File.write(File.join(dir, "xfile.rb"), "RcslX = 1")

require File.join(dir, "xfile.rb")
p Object.const_source_location(:RcslX).last        # 1 (defined in xfile.rb)
Object.send(:remove_const, :RcslX)
::RcslX = 2; here = __LINE__
p Object.const_source_location(:RcslX).last        # here — the redefinition line
p Object.const_source_location(:RcslX).first.end_with?("xfile.rb")  # false
FileUtils.rm_rf(dir)
