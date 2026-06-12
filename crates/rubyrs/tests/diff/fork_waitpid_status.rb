# Kernel#fork / Process.fork (block form) + Process.waitpid + $?
# — real fork(2) under the process-spawn capability. minitest's
# autorun fork-exit-status tests are the motivating consumer.
# The child runs ONLY its block (inherited frames are truncated,
# so an `exit` inside the block can't be swallowed by an enclosing
# rescue) and then drains its inherited at_exit handlers.

p Process.respond_to?(:fork)
p Kernel.private_method_defined?(:fork) || respond_to?(:fork, true)

pid = fork { exit 42 }
p pid.is_a?(Integer)
Process.waitpid(pid)
p $?.exitstatus
p $?.success?
p $?.pid == pid

# Plain block completion -> status 0.
Process.waitpid(Process.fork { :done })
p $?.exitstatus
p $?.success?

# Child output lands once (parent buffers flushed pre-fork).
print "parent-"
pid3 = fork { puts "child" }
Process.wait(pid3)
p $?.exitstatus

# An enclosing rescue must NOT see the child's exit.
begin
  Process.waitpid(fork { exit 7 })
  p $?.exitstatus
rescue SystemExit
  p :swallowed
end

# Child sees its own pid; parent pid unchanged.
parent_pid = Process.pid
r, w = nil, nil
pid4 = fork { exit(Process.pid == parent_pid ? 1 : 0) }
Process.waitpid(pid4)
p $?.exitstatus
p Process.pid == parent_pid
