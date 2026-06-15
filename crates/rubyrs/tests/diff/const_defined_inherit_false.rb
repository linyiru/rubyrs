# `Module#const_defined?(name, false)` — the `inherit=false` form checks
# ONLY the receiver's own constants, not the superclass chain / Object /
# top-level. Surfaced by stdlib uri/common.rb's
# `remove_const(sym) if const_defined?(sym, false)` load-time loop.
class Parent
  PCON = 1
end
class Child < Parent
end

# Own constant: true either way.
class Child; OWN = 2; end
p Child.const_defined?(:OWN, false)   # true
p Child.const_defined?(:OWN, true)    # true

# Inherited constant: true with inherit, FALSE without.
p Child.const_defined?(:PCON, true)   # true
p Child.const_defined?(:PCON, false)  # false  <-- the fix
p Child.const_defined?(:PCON)         # true (default inherit)

# Undefined: false either way.
p Child.const_defined?(:NOPE, false)  # false
p Child.const_defined?(:NOPE, true)   # false

# A module doesn't see a top-level constant with inherit=false.
TOPLEVEL_C = 9
module M; end
p M.const_defined?(:TOPLEVEL_C, true)   # true (inherits Object)
p M.const_defined?(:TOPLEVEL_C, false)  # false
