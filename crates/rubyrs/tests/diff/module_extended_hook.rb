# Module.extended(base) — fires on every `obj.extend(M)` and
# every `Class.extend(M)` call. Completes the included/prepended
# /extended hook triple. Receiver of the hook is the module being
# extended; argument is the receiver being extended (an Object
# for `obj.extend`, a Class for class-body / explicit Class.extend).

# (1) Object#extend — `obj.extend(M)` fires `M.extended(obj)`.
module MA
  def self.extended(base)
    puts "MA.extended(#{base.class.name})"
  end
end
o = Object.new
o.extend(MA)
# (singleton_class introspection isn't in the rubyrs subset;
# the hook fire is the observable side anyway.)

# (2) Class-body extend — `class Foo; extend M; end` fires
# `M.extended(Foo)`. Hook arg is the Class.
module MB
  def self.extended(base)
    puts "MB.extended(#{base.name})"
  end
end
class FooB
  extend MB
end

# (3) Explicit-receiver Class.extend.
module MC
  def self.extended(base)
    puts "MC.extended(#{base.name})"
  end
end
class FooC; end
FooC.extend(MC)

# (4) CRuby fires the hook on EVERY extend call, even when the
# chain insertion would be a no-op (idempotent re-extend). The
# hook isn't gated on chain change.
module MD
  def self.extended(base)
    puts "MD.extended(#{base.class.name})"
  end
end
od = Object.new
od.extend(MD)
od.extend(MD)        # fires again

# (5) Hook receiver is the module — `self == ME` inside.
module ME
  def self.extended(base)
    puts "self == ME : #{self == ME}"
    puts "base.is_a?(Object) : #{base.is_a?(Object)}"
  end
end
oe = Object.new
oe.extend(ME)

# (6) No hooks defined — silent no-op (CRuby doesn't raise).
module MF
  # No extended override.
end
of = Object.new
of.extend(MF)                                  # no output, no raise

# (7) Multi-arg `extend` — CRuby walks args RIGHT-to-LEFT, so
# M2 ends up at the eigenclass chain head and its hook fires
# FIRST, followed by M1. Both Object#extend and class-body
# extend share this order.
module MX1
  def self.extended(base); puts "MX1.extended(#{base.class.name})"; end
end
module MX2
  def self.extended(base); puts "MX2.extended(#{base.class.name})"; end
end
om = Object.new
om.extend(MX1, MX2)

# Class-body extend multi-arg — same right-to-left iteration.
module MY1
  def self.extended(base); puts "MY1.extended(#{base.name})"; end
end
module MY2
  def self.extended(base); puts "MY2.extended(#{base.name})"; end
end
class FooMulti
  extend MY1, MY2
end

# (8) respond_to? on extended target reflects the extend.
module MG
  def greeting; "hi from MG"; end
end
og = Object.new
og.extend(MG)
puts og.respond_to?(:greeting)        # true
puts og.greeting                      # hi from MG
