# Kernel#puts/print/p/warn must route through a reassigned
# $stdout/$stderr (minitest's capture_io swaps in a StringIO).
require "stringio"
orig = $stdout
cap = StringIO.new(+"")
$stdout = cap
puts "hello"
print "a", "b"
p [1, :two]
puts ["x", ["y", "z"]]
puts
$stdout = orig
puts "captured: #{cap.string.inspect}"
orige = $stderr
cape = StringIO.new(+"")
$stderr = cape
warn "danger"
warn "multi", "line"
$stderr = orige
puts "captured-err: #{cape.string.inspect}"
# dup'ed real stdout stays native (no infinite loop, still prints).
# Explicit flush before swapping back: under a pipe (CI capture)
# CRuby buffers the dup'ed IO independently of the original, and
# without the flush the at-exit flush order of the two buffers is
# platform-dependent — the fixture lines came out swapped on CI.
$stdout = orig.dup
puts "via-dup"
$stdout.flush
$stdout = orig
puts "after-restore"
