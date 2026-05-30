# Object#extend(Mod, ...) — install each Module into the
# receiver's eigenclass so M's instance methods become callable
# on `obj` directly. The Class-receiver path already existed
# (and `obj.extend` on a Class-shaped receiver continues to
# work via the pre-existing arm); this PR adds the plain
# Value::Object path.
#
# Pairs with PR #303's Object#singleton_methods: extended
# modules' instance methods now surface in
# `obj.singleton_methods` too, matching CRuby.

module M
  def m_hi
    "from M"
  end
end

module N
  def n_hi
    "from N"
  end
end

# Single module — method becomes callable, surfaces in
# singleton_methods.
o = Object.new
o.extend(M)
puts o.m_hi
puts o.singleton_methods.sort.inspect

# Multi-arg extend installs each in order
o2 = Object.new
o2.extend(M, N)
puts o2.m_hi
puts o2.n_hi
puts o2.singleton_methods.sort.inspect

# Idempotent — re-extending the same module doesn't double-add
o.extend(M)
puts o.singleton_methods.sort.inspect

# Transitive includes: if Q includes P, extending Q exposes
# both Q's and P's methods.
module P
  def p_hi
    "from P"
  end
end
module Q
  include P
  def q_hi
    "from Q"
  end
end
o3 = Object.new
o3.extend(Q)
puts o3.p_hi
puts o3.q_hi
puts o3.singleton_methods.sort.inspect

# `def obj.foo` and `obj.extend(M)` co-exist; both surface in
# singleton_methods.
o4 = Object.new
def o4.sing; "sing"; end
o4.extend(M)
puts o4.singleton_methods.sort.inspect

# extend returns the receiver (for chaining)
ret = Object.new.extend(M)
puts ret.m_hi
puts ret.is_a?(Object)

# Non-Module argument → TypeError. CRuby distinguishes "wrong
# argument type Integer" from "wrong argument type Class"; we
# follow.
begin
  Object.new.extend(42)
rescue TypeError
  puts "type-error-int"
end

begin
  Object.new.extend(String)   # Class is not a Module
rescue TypeError
  puts "type-error-class"
end

# respond_to? agrees with dispatch
puts Object.new.respond_to?(:extend)

# Zero-arg extend raises ArgumentError, NOT NoMethodError
# (cycle-1 review of this PR — was falling through to dispatch
# lookup and surfacing as NoMethodError).
begin
  Object.new.extend
rescue ArgumentError
  puts "argerr-zero-args"
end

# Transitive prepends — `Q prepends P; obj.extend(Q)` exposes
# P's methods in singleton_methods. Pre-cycle-1 walk only
# followed `includes`, missing prepended chains even though
# dispatch could call P's methods (cycle-1 fix follows
# Module#ancestors which spans both chains).
module Pp
  def pp_hi
    "Pp"
  end
end
module Qq
  prepend Pp
  def qq_hi
    "Qq"
  end
end
op = Object.new
op.extend(Qq)
puts op.pp_hi
puts op.singleton_methods.sort.inspect

# Methods inherited from extended modules participate in
# dispatch's normal precedence: own def > singleton > extended.
class C; def m_hi; "C-version"; end; end
c = C.new
c.extend(M)
puts c.m_hi    # "C-version" — own def wins
puts c.singleton_methods.include?(:m_hi)   # true — surfaced

# But singleton def takes precedence over both
def c.m_hi; "singleton-version"; end
puts c.m_hi
