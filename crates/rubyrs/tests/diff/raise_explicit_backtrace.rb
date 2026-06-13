# `raise Class, msg, backtrace` — the explicit 3rd backtrace argument
# is stamped onto the exception (CRuby semantics), so `e.backtrace`
# returns it verbatim rather than the raise-site frames. An empty `[]`
# is honoured too (it's non-nil, so the unwind's "already set" guard
# leaves it intact). rack's ShowExceptions renders "unknown location"
# for the empty-backtrace case and feeds e.backtrace through its frame
# parser otherwise — both depend on this. Zero require: exercises the
# 3-arg-raise desugar in compiler.rs.

# Explicit empty backtrace.
begin
  raise RuntimeError, "boom", []
rescue => e
  p e.backtrace
  p e.message
  p e.is_a?(RuntimeError)
end

# Explicit backtrace lines survive verbatim.
begin
  raise ArgumentError, "bad", ["app.rb:10:in 'run'", "lib.rb:3:in 'call'"]
rescue => e
  p e.backtrace
  p e.class.name
end

# nil 3rd arg → fall back to the call-site backtrace (an Array).
begin
  raise RuntimeError, "z", nil
rescue => e
  p e.backtrace.is_a?(Array)
end

# 2-arg form unchanged: message set, backtrace from the call site.
begin
  raise TypeError, "two"
rescue => e
  p e.message
  p(e.backtrace.is_a?(Array) && !e.backtrace.empty?)
end

# Re-raising an exception whose backtrace was set keeps it.
err = begin
  raise RuntimeError, "first", ["x.rb:1"]
rescue => e
  e
end
begin
  raise err
rescue => e2
  p e2.backtrace
end

# set_backtrace round-trips a String into a one-element Array.
begin
  raise RuntimeError, "s", "single.rb:9"
rescue => e
  p e.backtrace
end
