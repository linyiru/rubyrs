# Kernel#system under $stdout/$stderr reopen-delegation
# (capture_subprocess_io): the child's pipes are captured and
# forwarded through the veneer's Ruby-level write — Tempfile-backed
# delegates have no real fd to hand the child. stdlib-gated
# (Tempfile) and spawn-gated (CLI default allows it).
require "tempfile"

cap_out = Tempfile.new("out")
cap_err = Tempfile.new("err")
orig_out = $stdout.dup
orig_err = $stderr.dup
$stdout.reopen cap_out
$stderr.reopen cap_err
ok1 = system "echo hi"
ok2 = system "echo bye! 1>&2"
$stdout.rewind
$stderr.rewind
got_out = cap_out.read
got_err = cap_err.read
$stdout.reopen orig_out
$stderr.reopen orig_err

p ok1
p ok2
p got_out
p got_err.strip
# Plain system still works when nothing is redirected.
p system("true")
