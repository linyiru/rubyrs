# Module#define_method / Object#define_singleton_method —
# 2-arg Proc/Method/UnboundMethod form. Pre-PR rubyrs raised
# ArgumentError "not yet supported by rubyrs Tier-1" for the
# 2-arg shape; this fixture pins the four common paths.
#
# Source types supported:
#   * Proc (Value::Block)           — captured proto + closure
#   * BoundMethod (Value::BoundMethod) — install snapshot
#   * UnboundMethod (Value::UnboundMethod) — bind compat check
#                                          + install snapshot

# (1) Proc form, explicit receiver
class C; end
puts C.define_method(:dbl, proc { |x| x * 2 })
puts C.new.dbl(5)

# (2) Proc form, bare call inside a class body (no_recv path)
class D
  define_method(:add100, proc { |y| y + 100 })
end
puts D.new.add100(5)

# (3) Closure capture survives install (CRuby parity — proc
# carries its lexical bindings into the installed method).
counter = 0
class CC; end
CC.define_method(:bump, proc { counter += 1 })
3.times { CC.new.bump }
puts counter

# (4) Object#define_singleton_method with Proc — Object recv
o = Object.new
o.define_singleton_method(:foo, proc { "singleton-proc" })
puts o.foo

# (5) Object#define_singleton_method with Proc — Class recv
class K; end
K.define_singleton_method(:cls_foo, proc { "K.cls_foo" })
puts K.cls_foo

# (6) define_method via __send__ (runtime arm) — Sinatra-
# style dynamic-name install
class S; end
S.__send__(:define_method, :hi, proc { "S.hi" })
puts S.new.hi

# (7) BoundMethod source — install another object's method
class P; def k; "P.k"; end; end
m = P.new.method(:k)
class P2 < P; end
P2.define_method(:k2, m)
puts P2.new.k2

# (8) UnboundMethod source — bind-compat check at install time
class A; def aa; "A.aa"; end; end
class A2 < A; end
um = A.instance_method(:aa)
A2.define_method(:aa_alias, um)
puts A2.new.aa_alias

# (9) UnboundMethod from unrelated hierarchy → TypeError
# matching CRuby's "bind argument must be a subclass of X"
# wording.
class Unrelated; def x; end; end
um_un = Unrelated.instance_method(:x)
begin
  C.define_method(:x_bad, um_un)
rescue TypeError => e
  puts e.message.start_with?("bind argument must be a subclass of Unrelated")
end

# (10) respond_to? on the installed method
puts C.new.respond_to?(:dbl)
puts D.new.respond_to?(:add100)
puts o.respond_to?(:foo)

# (11) Cycle-1: visibility handling — bare define_method inside
# a `private` block inherits private; explicit-receiver call
# from inside a `private` block in an unrelated class must NOT
# leak the caller's visibility onto the target class.
class Priv
  private
  define_method(:bare_2arg, proc { "B" })
end
puts Priv.private_instance_methods(false).include?(:bare_2arg)
puts Priv.public_instance_methods(false).include?(:bare_2arg)

class Target; end
class Caller
  private
  Target.define_method(:safe_x, proc { "x" })
end
# Target.safe_x is Public despite Caller's surrounding `private`
puts Target.public_instance_methods(false).include?(:safe_x)
puts Target.private_instance_methods(false).include?(:safe_x)

# (12) Cycle-2: Module-owned UnboundMethods bind universally
# (CRuby parity — Module.instance_method(:m).bind(obj) succeeds
# regardless of obj's class hierarchy, mirroring the
# existing UnboundMethod#bind fence).
module Mod_bind
  def universal_m
    "universal"
  end
end
class Recv_unrelated; end
um_mod = Mod_bind.instance_method(:universal_m)
Recv_unrelated.define_method(:from_mod, um_mod)
puts Recv_unrelated.new.from_mod

# Kernel UnboundMethods are likewise universally bindable.
class Recv_kern; end
um_kern = Kernel.instance_method(:object_id)
Recv_kern.define_method(:my_oid, um_kern)
puts Recv_kern.new.my_oid.is_a?(Integer)
