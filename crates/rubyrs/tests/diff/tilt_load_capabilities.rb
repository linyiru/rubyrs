# Capability stubs added so heavy gems (tilt, etc.) can finish
# loading without rubyrs choking on Module-level lifecycle calls
# that don't change runtime behaviour in our embeddable subset.
#
# This fixture only pins the CRuby-MATCHING surface (so it can
# diff byte-for-byte). The documented divergences — `frozen?`
# returning false on a frozen array, `defined?` reporting
# "expression" instead of "constant" for autoloaded names,
# `class << self; prepend(...)` raising NotImplementedError
# instead of running — are intentional gaps; pinning them here
# would just lock in divergence. They're documented at the
# implementation sites and in SUBSET.md.

# --- private_constant / public_constant ---
# Both forms (no-recv inside class body, explicit receiver from
# outside) accept symbol args and return the module (chainable).
module CV
  HIDDEN = 1
  private_constant :HIDDEN
  VISIBLE = 2
  public_constant :VISIBLE
end
puts CV::VISIBLE                                # 2 (visibility not enforced)
puts(CV.private_constant(:HIDDEN) == CV)        # true (chainable form)

# --- autoload ---
# Returns nil (CRuby's actual contract for `Module#autoload`,
# not the chainable shape some might expect).
module AL
end
puts AL.autoload(:Maybe, "/some/path").inspect  # nil

# --- Array / Hash freeze ---
# Returns the receiver (chainable). Used by patterns like
# `EMPTY_ARRAY = [].freeze` and `EMPTY_HASH = {}.freeze`.
a = [1, 2, 3].freeze
puts a.inspect                                  # [1, 2, 3]
h = {a: 1, b: 2}.freeze
puts h.inspect                                  # {a: 1, b: 2}

# --- respond_to? 2-arg form ---
# Feature-detection idiom: `respond_to?(:foo, true)` checks
# even private methods. Previously NoMethodError on the 2-arg
# shape; now accepted.
class RC
  def public_one; end
  private
  def private_one; end
end
r = RC.new
puts r.respond_to?(:public_one)                 # true
puts r.respond_to?(:public_one, true)           # true
puts r.respond_to?(:nonexistent, false)         # false
