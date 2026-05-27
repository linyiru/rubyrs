# `Class#singleton_class` — Tier 1 stub returning the receiver.
#
# CRuby's real metaclass (eigenclass) has its own identity and
# its `instance_methods` are the original's singleton_methods.
# rubyrs's stub returns the receiver itself: the identity-
# invariant property `X.singleton_class.equal?(X.singleton_class)`
# holds (both calls return X), which is the property real
# consumers actually need.
#
# Motivating use: MRI lib/erb/compiler.rb:828 + 900 caches
# `@_init = self.class.singleton_class` then later checks
# `@_init.equal?(self.class.singleton_class)` to detect class-
# level reopening — only the identity round-trip matters.
#
# DIVERGENCE (documented at the impl site): with the stub,
# `C.singleton_class.name` returns `"C"` (the original class
# name) instead of CRuby's nil (real singleton classes have no
# name). Methods called on the singleton_class result dispatch
# against the receiver, so the metaclass shape (where
# singleton_methods become instance_methods of the result)
# isn't visible.

# --- Idempotency: same call returns same object ---
class A; end
puts A.singleton_class.equal?(A.singleton_class)   # true

# --- Class of result is Class ---
puts A.singleton_class.class                        # Class

# --- Different classes have different singletons ---
class B; end
puts A.singleton_class.equal?(B.singleton_class)    # false

# --- ERB-shape probe ---
# Mirror lib/erb/compiler.rb's @_init cache invariant: cache
# the singleton_class on construction, then verify it on later
# operations.
class Tmpl
  def initialize
    @init = self.class.singleton_class
  end
  def consistent?
    @init.equal?(self.class.singleton_class)
  end
end
puts Tmpl.new.consistent?                           # true

# --- respond_to? consistency ---
puts A.respond_to?(:singleton_class)                # true
