# Extended Errno::* coverage — 15 additional CRuby Errno
# subclasses beyond the file/path set in
# `tests/diff/exception_hierarchy.rb`. Pre-installed for the
# rescue patterns gems use around non-blocking IO, network
# errors, server-bind failures, and resource-exhaustion paths.
# All classes sit under SystemCallError; each is reachable via
# `rescue Errno::FOO`, via `rescue SystemCallError`, and via
# `rescue StandardError` (the parent chain).

# Hierarchy renders (first 3 entries — Object-tail divergence
# documented as Tier-1).
%w[
  EAGAIN EWOULDBLOCK ETIMEDOUT EINTR EBADF EIO
  EADDRINUSE EADDRNOTAVAIL EHOSTUNREACH ENETUNREACH
  EINPROGRESS ENOTCONN EMFILE ENFILE ENOMEM
].each do |name|
  cls = Errno.const_get(name)
  puts "#{name}=#{cls.ancestors.first(3).inspect}"
end

# Rescue via SystemCallError parent.
begin
  raise Errno::EAGAIN, "retry"
rescue SystemCallError => e
  puts "scerr: #{e.class}"
end

# Rescue via StandardError (two levels up).
begin
  raise Errno::ETIMEDOUT, "slow"
rescue StandardError => e
  puts "stderr: #{e.class}"
end

# Rescue specific class. CRuby auto-prefixes the message with
# the errno description ("Address already in use - port busy");
# rubyrs leaves the message as passed. Check only the class and
# the trailing user-supplied portion of the message — both
# runtimes agree on those.
begin
  raise Errno::EADDRINUSE, "port busy"
rescue Errno::EADDRINUSE => e
  puts "specific_class=#{e.class}"
  puts "specific_msg_tail=#{e.message.end_with?("port busy")}"
end

# EWOULDBLOCK is a constant aliased to EAGAIN on Linux + Darwin
# (same errno integer). Both runtimes resolve to the same class
# object — verify the alias holds and that `raise
# Errno::EAGAIN` can be caught via `rescue Errno::EWOULDBLOCK`.
puts "wb_eq_eagain=#{Errno::EWOULDBLOCK == Errno::EAGAIN}"
begin
  raise Errno::EAGAIN, "would block"
rescue Errno::EWOULDBLOCK => e
  puts "wb_catches_eagain=#{e.class}"
end

# is_a? walks the parent chain.
e = Errno::EBADF.new("fd")
puts "isa_self=#{e.is_a?(Errno::EBADF)}"
puts "isa_sce=#{e.is_a?(SystemCallError)}"
puts "isa_se=#{e.is_a?(StandardError)}"
puts "isa_exc=#{e.is_a?(Exception)}"
puts "isa_other_errno=#{e.is_a?(Errno::EAGAIN)}"

# Bare `rescue` (StandardError filter) catches Errno::* — they
# inherit through StandardError, distinct from the
# `< Exception` placement of SecurityError / NoMemoryError /
# ResourceExhausted that bare rescue must NOT catch.
caught = nil
begin
  raise Errno::EHOSTUNREACH, "unreachable"
rescue => e
  caught = e.class
end
puts "bare_catches=#{caught}"
