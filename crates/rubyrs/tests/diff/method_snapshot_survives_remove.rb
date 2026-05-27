# `Method` / `UnboundMethod` snapshot the resolved Method at
# capture time, so the bind/call path survives a subsequent
# `remove_method` on the captured class (CRuby parity).
#
# Without the snapshot field on BoundMethod and UnboundMethod,
# `bm.call` and `ubm.bind(x).call` did live class-chain lookup
# at call time — which raises NoMethodError once the entry has
# been stripped from the table, even though the handle was
# captured BEFORE the removal. This file locks the parity for
# four capture-then-remove-then-call shapes:
#
# 1. `obj.method(:foo).call` after `remove_method(:foo)`
# 2. `obj.method(:foo).unbind.bind(obj).call` round-trip
# 3. `Class.instance_method(:foo).bind(obj).call`
# 4. `Class.instance_method(:foo).bind_call(obj)` (regression
#    case — this path was already locked by the original PR,
#    here just for completeness alongside the new BoundMethod
#    paths).

# --- (1) BoundMethod#call after remove ---
class A
  def foo; "a-foo"; end
end
a = A.new
bm = a.method(:foo)
A.class_eval { remove_method(:foo) }
puts bm.call                                 # a-foo

# --- (2) unbind → bind → call round-trip across remove ---
class B
  def bar; "b-bar"; end
end
b = B.new
bm2 = b.method(:bar)
um2 = bm2.unbind
B.class_eval { remove_method(:bar) }
puts um2.bind(b).call                        # b-bar

# --- (3) instance_method → bind → call after remove ---
class C
  def baz; "c-baz"; end
end
c = C.new
um3 = C.instance_method(:baz)
bm3 = um3.bind(c)
C.class_eval { remove_method(:baz) }
puts bm3.call                                # c-baz

# --- (4) instance_method → bind_call after remove
#     (already covered by unbound_method_bind_call.rb;
#     locked here too for the symmetry sanity check) ---
class D
  def qux; "d-qux"; end
end
d = D.new
um4 = D.instance_method(:qux)
D.class_eval { remove_method(:qux) }
puts um4.bind_call(d)                        # d-qux

# --- (5) Singleton method capture: `obj.method(:foo)` must
#     snapshot from the dispatch class (singleton chain), NOT
#     the script-visible `obj.class`. Otherwise the snapshot
#     points at the real class's body and `bm.call` invokes
#     the wrong implementation.
class E
  def beep; "class-beep"; end
end
e = E.new
def e.beep; "singleton-beep"; end
bm5 = e.method(:beep)
puts bm5.call                                # singleton-beep

# --- (6) Singleton-method unbind fence: `c.method(:foo).unbind`
#     when `foo` is a singleton method on `c` produces an
#     UnboundMethod whose captured class is the eigenclass.
#     `bind(another_real_class_instance)` must raise TypeError —
#     singleton methods only belong to the original instance.
#     Without the eigenclass-aware capture, this would silently
#     invoke the singleton body on the wrong receiver.
class G
  def boop; "class-boop"; end
end
g1 = G.new
def g1.boop; "singleton-boop"; end
# --- Introspection paths (arity, source_location, owner) also
#     prefer the snapshot. Without it, asking for arity AFTER
#     remove_method would return -1 (the variadic "method not
#     found" fallback) and source_location would return nil
#     even though bm.call still works.
class H
  def kweep(x, y); x + y; end
end
um_h = H.instance_method(:kweep)
H.class_eval { remove_method(:kweep) }
puts um_h.arity                              # 2
puts um_h.source_location.is_a?(Array)       # true
puts um_h.owner == H                         # true

um7 = g1.method(:boop).unbind
# Positive: binding back to the ORIGINAL instance succeeds.
# Without an eigenclass-aware target_class derivation in bind /
# bind_call, this would fail too — the original instance's
# real class doesn't walk through its own singleton class.
puts um7.bind(g1).call                       # singleton-boop
puts um7.bind_call(g1)                       # singleton-boop
# Negative: binding to a DIFFERENT instance raises TypeError.
g2 = G.new
begin
  um7.bind(g2).call
  puts "no raise (BAD)"
rescue TypeError
  puts "singleton-unbind + wrong-recv → TypeError"
end

# --- (7) Implicit-self `method(:foo)` also snapshots from the
#     dispatch class — same singleton-respecting rule.
class F
  def boop; "class-boop"; end
  def capture_self
    method(:boop)
  end
end
f = F.new
def f.boop; "singleton-boop"; end
bm6 = f.capture_self
puts bm6.call                                # singleton-boop
