# Method#== / UnboundMethod#==
# - BoundMethod: same receiver identity AND same method name.
# - UnboundMethod: same underlying Method definition (so a method
#   inherited by a subclass equals the parent's UnboundMethod).

class C
  def foo; end
  def bar; end
end
class D < C
end

c1 = C.new
c2 = C.new
d1 = D.new

# Same recv + same name → true (different .method calls produce
# different BoundMethod ObjIds but equal value).
puts c1.method(:foo) == c1.method(:foo)   # true

# Different recv of the same class → false.
puts c1.method(:foo) == c2.method(:foo)   # false

# Same recv, different name → false.
puts c1.method(:foo) == c1.method(:bar)   # false

# Subclass instance inherits foo but its recv differs from c1 — false.
puts c1.method(:foo) == d1.method(:foo)   # false

# Cross-type — false, no NoMethodError.
puts c1.method(:foo) == 42                # false

# Primitive recv compares by value: 7.method(:+) on two literal
# 7s is the "same" Integer.
puts 7.method(:+) == 7.method(:+)         # true

# UnboundMethod equality: same underlying definition. (We reach
# UnboundMethods via `bm.unbind` since `Class#instance_method`
# isn't in the subset.)
u_foo_via_c1 = c1.method(:foo).unbind
u_foo_via_c2 = c2.method(:foo).unbind
u_foo_via_d  = d1.method(:foo).unbind
u_bar        = c1.method(:bar).unbind

puts u_foo_via_c1 == u_foo_via_c2   # true
puts u_foo_via_c1 == u_foo_via_d    # true — D inherits :foo from C
puts u_foo_via_c1 == u_bar          # false

# Method#eql? is an alias of Method#== (CRuby parity). Without
# the explicit eql? arm, this would reach the universal
# ruby_eq fallback (no Method case → false) and disagree with
# `==`.
puts c1.method(:foo).eql?(c1.method(:foo))     # true
puts c1.method(:foo).eql?(c2.method(:foo))     # false
puts c1.method(:foo).eql?(c1.method(:bar))     # false
puts c1.method(:foo).eql?(42)                  # false
puts u_foo_via_c1.eql?(u_foo_via_d)            # true
puts u_foo_via_c1.eql?(u_bar)                  # false

# Method#!= — negation of ==. Without an explicit arm, the
# universal `!=` fallback (via ruby_eq → false) would return
# true for two equivalent Methods.
puts c1.method(:foo) != c1.method(:foo)        # false
puts c1.method(:foo) != c2.method(:foo)        # true
puts c1.method(:foo) != c1.method(:bar)        # true
puts u_foo_via_c1 != u_foo_via_d               # false

# Wrong-arity raises ArgumentError (CRuby parity — not
# NoMethodError via dispatch fall-through).
begin
  c1.method(:foo).eql?(1, 2)
rescue ArgumentError => e
  puts e.message
end

# respond_to? must agree.
puts c1.method(:foo).respond_to?(:eql?)        # true
puts c1.method(:foo).respond_to?(:!=)          # true
puts u_foo_via_c1.respond_to?(:eql?)           # true
puts u_foo_via_c1.respond_to?(:!=)             # true

# Redefine detection — after `def foo` re-runs on the source
# class, an OLD BoundMethod captured before the redefine no
# longer compares equal to a freshly-captured NEW one. CRuby
# parity (Method#== checks the underlying Method/iseq); rubyrs
# routes through snapshot-Rc identity. Same rule applies to
# UnboundMethod via `.unbind` round-trips, so the BoundMethod ↔
# UnboundMethod boundary stays symmetric.
class R
  def foo; "v1"; end
end
r = R.new
old_bm = r.method(:foo)
old_um = old_bm.unbind
class R
  def foo; "v2"; end       # redefine
end
new_bm = r.method(:foo)
new_um = new_bm.unbind
puts old_bm == new_bm            # false — different Method snapshots
puts old_bm.eql?(new_bm)         # false
puts old_um == new_um            # false — same rule via UnboundMethod
puts new_bm == r.method(:foo)    # true — two captures of v2 share the table Rc
puts new_bm.hash == r.method(:foo).hash   # true — hash lock-step

# Hash invariant — eql?-equal Methods must share #hash (Ruby's
# `a.eql?(b) ⇒ a.hash == b.hash`). This is the rule that makes
# Method usable as a Hash key; we exercise the rule directly
# rather than via Hash lookup because the rubyrs Hash internals
# use a separate ruby_eql identity for key comparison (full
# Method-as-Hash-key parity is a separate follow-up).
puts c1.method(:foo).hash == c1.method(:foo).hash    # true
puts u_foo_via_c1.hash == u_foo_via_c2.hash          # true
puts u_foo_via_c1.hash == u_foo_via_d.hash           # true — D inherits :foo
