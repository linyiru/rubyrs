# Hash#compare_by_identity flag + compare_by_identity? + dup/clone
# propagation + the frozen guard. Identity-comparison semantics
# already hold for object/class/module keys (rubyrs's Tier-1 Hash),
# which is the realistic identity-map use (zeitwerk's Cref::Map is
# keyed by Module objects).

h = {}
p h.compare_by_identity?           # false
r = h.compare_by_identity
p r.equal?(h)                      # true — returns self
p h.compare_by_identity?           # true

class A; end
class B; end
h[A] = 1
h[B] = 2
p h[A]
p h[B]
p h.size

# dup and clone both preserve the flag (CRuby copies it on each).
p h.dup.compare_by_identity?       # true
p h.clone.compare_by_identity?     # true

# Symbol keys stay distinct/identity-stable under the flag.
g = {}.compare_by_identity
g[:x] = 10
p g[:x]
p g.compare_by_identity?

# Frozen Hash raises FrozenError on the mutator.
begin
  {}.freeze.compare_by_identity
rescue FrozenError => e
  p :frozen_error
end
