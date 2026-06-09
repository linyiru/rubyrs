# `$!` (errinfo) is dynamically scoped like CRuby: it holds the
# in-flight exception only WHILE a rescue/ensure body runs, then
# reverts to its prior value when the begin region is left.
# (NOTE: `p $!` / `$!.inspect` has an unrelated pre-existing message
# divergence, so this fixture probes via .class/.message/.nil?.)

# 1. after a rescue completes normally → $! reverts to nil
begin; raise "x"; rescue; end
p $!.nil?                       # true

# 2. $! is the in-flight exception DURING the rescue body
begin
  raise ArgumentError, "boom"
rescue
  p $!.class                    # ArgumentError
  p $!.message                  # "boom"
end
p $!.nil?                       # true again afterwards

# 3. nested rescue → inner restores $! to the OUTER exception
begin
  raise "outer"
rescue
  begin; raise "inner"; rescue; end
  p $!.message                  # "outer"  (not "inner", not nil)
end
p $!.nil?                       # true

# 4. a rescued `require` miss must not leak — a later bare `raise`
#    raises RuntimeError ("unhandled"), NOT the stale LoadError
begin; require "no_such_lib_zzz"; rescue LoadError; end
p $!.nil?                       # true
begin
  raise
rescue => e
  p e.class                     # RuntimeError (not LoadError)
end

# 5. `return` out of a rescue body restores the caller's $!
def handled_then_return
  begin
    raise "inside"
  rescue
    return 42
  end
end
p handled_then_return           # 42
p $!.nil?                       # true — callee's handled exc didn't leak

# 6. method-entry $! is preserved across an internal handled exception
def inner_handles
  begin; raise "y"; rescue; return :ok; end
end
begin
  raise "keep_me"
rescue
  inner_handles
  p $!.message                  # "keep_me" — restored after the call
end
p $!.nil?                       # true

# 7. ensure body sees the in-flight exception; it still propagates
$ensure_seen = nil
def with_ensure
  begin
    raise "e1"
  ensure
    $ensure_seen = $!&.message
  end
end
begin; with_ensure; rescue; end
p $ensure_seen                  # "e1"
p $!.nil?                       # true
