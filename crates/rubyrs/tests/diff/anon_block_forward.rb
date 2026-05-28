# Anonymous block forwarding (Ruby 3.1+):
#   def foo(&)
#     inner(&)
#   end
# The parameter list `(&)` accepts a block but doesn't bind it
# to a name; the matching call site `(&)` forwards the captured
# block. Gapscan #1 cross-codebase pattern (sinatra / dry-struct
# / Tilt all use the named-`&blk` form already, but several
# modern gems have started adopting the anonymous form).
#
# rubyrs implementation: bind the anonymous `(&)` parameter to
# the reserved local name `&` (invalid as a user identifier);
# at the call site, rewrite `inner(&)` to a CallWithBlockArg
# whose block_arg expression is `LVarRead("&")` — i.e. read
# that sentinel local and forward it as the block argument.
# Reuses the existing named-`&blk` forwarding plumbing; no new
# opcodes, no runtime changes.

# --- (1) Basic forwarding ---
def inner1(&blk)
  blk.call(2)
end
def anon1(&)
  inner1(&)
end
puts anon1 { |x| "a=#{x}" }                   # a=2

# --- (2) Forwarding chain: anon → anon → named ---
def named3(&blk)
  blk.call("end")
end
def anon3(&)
  named3(&)
end
def anon3outer(&)
  anon3(&)
end
puts anon3outer { |x| "chain=#{x}" }          # chain=end

# --- (3) Anonymous capture without forwarding ---
# `def foo(&); end` should accept a block silently (no bind,
# no use); CRuby simply ignores it. Verifies the param parse
# doesn't break either path.
def silent_anon(&)
  "ok"
end
puts silent_anon { "ignored" }                # ok
puts silent_anon                              # ok (no block also fine)

# --- (4) Mixed with explicit args ---
def with_args(x, y, &)
  combine(x, y, &)
end
def combine(a, b, &blk)
  blk.call(a + b)
end
puts with_args(3, 4) { |s| "sum=#{s}" }       # sum=7

# --- (5) Method#parameters / arity introspection on the sentinel ---
# CRuby reports the anonymous block as `[[:block, :&]]` — the
# literal `:&` Symbol. Our sentinel implementation passes
# through unchanged, giving byte-for-byte parity.
class Intro
  def named(&blk); end
  def anon(&); end
end
puts Intro.instance_method(:named).parameters.inspect  # [[:block, :blk]]
puts Intro.instance_method(:anon).parameters.inspect   # [[:block, :&]]
puts Intro.instance_method(:named).arity               # 0
puts Intro.instance_method(:anon).arity                # 0

# --- (6) Forwarding through yield-style consumer ---
def yielder
  yield 10
end
def anon_yield(&)
  yielder(&)
end
puts anon_yield { |v| "y=#{v}" }              # y=10
