# $stdout.reopen(tempfile) delegation round-trip — minitest's
# capture_subprocess_io shape: dup, reopen onto a Tempfile, write
# through Kernel#puts AND $stdout.print, rewind, read back, reopen
# the dup to restore.
require "tempfile"
cap = Tempfile.new("out")
orig = $stdout.dup
$stdout.reopen cap
puts "into-capture"
$stdout.print "more"
$stdout.rewind
got = cap.read
$stdout.reopen orig
orig.close
cap.close!
puts "captured: #{got.inspect}"
puts "back-to-normal"
