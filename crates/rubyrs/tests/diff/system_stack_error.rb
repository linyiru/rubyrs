# SystemStackError — CRuby raises a catchable Ruby exception when
# method recursion exceeds the default depth limit. Before the
# stack-depth guard landed, rubyrs had no ceiling: a runaway
# recursion (e.g. the alias_method-into-feedback-loop sinatra-
# contrib/WebDAV's modular-form double-`register` produces) just
# kept allocating frames on the heap until the OS OOM-killer
# fired — observed at >90 GB resident memory before the kill in
# one terminal session. Catching it as `SystemStackError` instead
# matches CRuby's contract.

# Direct infinite recursion via a self-call.
def loop_forever; loop_forever; end
begin
  loop_forever
rescue SystemStackError => e
  puts "caught_self_recursion=#{e.class} message=#{e.message}"
end

# Mutual recursion across two methods — still trips the same cap.
def ping; pong; end
def pong; ping; end
begin
  ping
rescue SystemStackError => e
  puts "caught_mutual=#{e.class}"
end

# Bare `rescue` (StandardError) MUST NOT swallow SystemStackError
# — CRuby places it directly under Exception specifically for
# this reason. Verifies the exception hierarchy placement.
caught_at_bare = nil
caught_at_outer = nil
begin
  begin
    loop_forever
  rescue => e
    caught_at_bare = e.class
  end
rescue SystemStackError => e
  caught_at_outer = e.class
end
puts "bare_rescue_caught=#{caught_at_bare.inspect}"
puts "outer_explicit_caught=#{caught_at_outer}"

# `rescue Exception` is the broader catch-all that DOES include
# SystemStackError (since SystemStackError < Exception).
begin
  loop_forever
rescue Exception => e
  puts "rescue_Exception_class=#{e.class}"
end

# The exception responds to .message (inherited from Exception).
begin
  loop_forever
rescue SystemStackError => e
  puts "message_match=#{e.message == 'stack level too deep'}"
end

# Ancestor chain placement — SystemStackError < Exception, NOT
# < StandardError. (Don't render the full ancestor list because
# the Object-tail differs between runtimes — a documented Tier-1
# divergence unrelated to this fix.)
puts "SSE_first_two=#{SystemStackError.ancestors.first(2).inspect}"
puts "SSE_is_StandardError=#{SystemStackError.ancestors.include?(StandardError)}"
puts "SSE_is_Exception=#{SystemStackError.ancestors.include?(Exception)}"

# After catching SystemStackError, the VM stays usable — the
# unwind happens cleanly, no host-side state corruption.
def factorial(n)
  n <= 1 ? 1 : n * factorial(n - 1)
end
puts "post_rescue_compute=#{factorial(10)}"
