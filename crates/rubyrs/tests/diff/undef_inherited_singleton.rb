# `obj.singleton_class.undef_method(:x)` for an INHERITED method makes
# the object stop responding to `x`, even though an ancestor (the
# native class) still defines it — CRuby's undef installs a tombstone
# on the eigenclass that shadows the inherited method. rack's Lint
# tests this: `obj = {}; obj.singleton_class.send(:undef_method,
# :to_hash); ... session.respond_to?(:to_hash)` must be false.

# Hash instance: undef an inherited native method
h = {}
p h.respond_to?(:to_hash)                       # true
h.singleton_class.send(:undef_method, :to_hash)
p h.respond_to?(:to_hash)                       # false

# a different Hash is unaffected (tombstone is per-instance)
p({}.respond_to?(:to_hash))                     # true

# undef of a user-defined singleton method (control)
h2 = {}
def h2.custom; 42; end
p h2.respond_to?(:custom)                        # true
h2.singleton_class.send(:undef_method, :custom)
p h2.respond_to?(:custom)                        # false

# Array instance: same tombstone semantics
a = [1, 2, 3]
p a.respond_to?(:first)                          # true
a.singleton_class.send(:undef_method, :first)
p a.respond_to?(:first)                          # false
p [9].respond_to?(:first)                        # true (other instance fine)

# String instance
s = "hi"
p s.respond_to?(:upcase)                         # true
s.singleton_class.send(:undef_method, :upcase)
p s.respond_to?(:upcase)                         # false
