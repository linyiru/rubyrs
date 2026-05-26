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

# --- respond_to? agrees with dispatch ---
# The whitelist in lookup.rs must stay in sync with the new
# dispatch stubs — otherwise `Array#freeze` works but
# `[].respond_to?(:freeze)` lies. Pin both directions.
puts [].respond_to?(:freeze)                    # true
puts [].respond_to?(:frozen?)                   # true
puts({}.respond_to?(:freeze))                   # true
puts({}.respond_to?(:frozen?))                  # true
module RtTest
end
puts RtTest.respond_to?(:autoload)              # true
puts RtTest.respond_to?(:private_constant)      # true
puts RtTest.respond_to?(:public_constant)       # true

# --- autoload arity ---
# Stubbed but still validates argc (2 required). Wrong arity
# raises ArgumentError, matching CRuby — the no-op stub doesn't
# swallow caller bugs.
begin
  RtTest.autoload(:OnlyOne)
rescue ArgumentError => e
  puts e.message
end

# --- NotImplementedError class hierarchy (CRuby parity) ---
# Subclass of ScriptError (NOT StandardError) so a bare `rescue`
# does NOT catch it — that's CRuby's behaviour and we now match.
# Important divergence to pin because the bare-rescue default
# is what most code relies on. Construct a probe that catches
# the bare-rescue miss with an explicit NotImplementedError
# rescue at an outer layer, then reports which one fired.
caught_at = begin
  begin
    raise NotImplementedError, "probe"
  rescue
    "bare"
  end
rescue NotImplementedError
  "explicit"
end
puts caught_at                                  # "explicit" in CRuby AND rubyrs

# Direct explicit rescue still catches it (sanity).
caught_explicit = begin
  raise NotImplementedError, "oops"
rescue NotImplementedError
  true
end
puts caught_explicit                            # true
