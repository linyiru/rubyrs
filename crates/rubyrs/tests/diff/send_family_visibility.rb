# The send-family visibility matrix (CRuby 3.4 probed):
#   - `public_send` calls ONLY public methods. Private/protected raise
#     the kind-appropriate NoMethodError with CRuby's exact message —
#     even for the literal-`self` receiver form and a protected call
#     from a type-compatible (kin) caller, both of which a normal
#     explicit-receiver call would allow.
#   - `send` / `__send__` bypass visibility entirely.
#   - A visibility-failed call routes to a user `method_missing`
#     (CRuby's default method_missing is what raises the error, so a
#     user override intercepts it).
#   - `respond_to?`'s include_all flag is evaluated for TRUTHINESS,
#     and the default (false) excludes BOTH private and protected.
# Known divergences deliberately NOT tested here: missing-method
# message quoting (rubyrs uses the backtick form), bare toplevel
# `public_send(:toplevel_def)` (toplevel defs aren't modeled as
# private Object methods), block-form `public_send { }` leniency,
# BasicObject receivers.

class Matrix
  def pub; :pub; end
  protected def prot; :prot; end
  private def priv; :priv; end

  # literal-self / kin-caller forms (normal calls allow these; the
  # strict public_send must still raise)
  def self_pub_send_priv
    public_send(:priv)
  rescue NoMethodError => e
    "raised: #{e.message}"
  end

  def explicit_self_pub_send_priv
    self.public_send(:priv)
  rescue NoMethodError => e
    "raised: #{e.message}"
  end

  def kin_pub_send_prot(other)
    other.public_send(:prot)
  rescue NoMethodError => e
    "raised: #{e.message}"
  end

  def kin_normal_prot(other)   # control: the normal call is allowed
    other.prot
  end

  def self_send_priv           # control: send bypasses
    send(:priv)
  end
end

m = Matrix.new
n = Matrix.new

puts "== public_send strictness =="
begin; m.public_send(:priv); rescue NoMethodError => e; puts "priv-sym: #{e.message}"; end
begin; m.public_send("priv"); rescue NoMethodError => e; puts "priv-str: #{e.message}"; end
begin; m.public_send(:prot); rescue NoMethodError => e; puts "prot-sym: #{e.message}"; end
begin; m.public_send("prot"); rescue NoMethodError => e; puts "prot-str: #{e.message}"; end
p m.public_send(:pub)
p m.public_send("pub")
p m.public_send(:pub) == m.pub

puts "== self/kin exemptions do NOT apply to public_send =="
puts m.self_pub_send_priv
puts m.explicit_self_pub_send_priv
puts m.kin_pub_send_prot(n)
p m.kin_normal_prot(n)
p m.self_send_priv

puts "== send / __send__ bypass =="
p m.send(:priv)
p m.send("priv")
p m.send(:prot)
p m.__send__(:priv)
p m.__send__(:prot)

puts "== args thread through =="
class Args
  def pub2(a, b); [a, b]; end
  private def priv1(a); a * 2; end
end
a = Args.new
p a.public_send(:pub2, 1, 2)
p a.send(:priv1, 21)
begin; a.public_send(:priv1, 21); rescue NoMethodError => e; puts "priv-args: #{e.message}"; end

puts "== visibility-failed call routes to method_missing =="
class Proxyish
  def method_missing(name, *args)
    "mm:#{name}:#{args.inspect}"
  end
  def respond_to_missing?(name, include_all = false)
    true
  end
  private def hidden(x); [:hidden, x]; end
  protected def guarded; :guarded; end
end
px = Proxyish.new
p px.public_send(:hidden, 5)     # private via public_send -> method_missing
p px.public_send(:guarded)       # protected via public_send -> method_missing
p px.public_send(:absent, 1)     # missing -> method_missing
p px.send(:hidden, 7)            # send bypasses -> real method

puts "== respond_to? visibility + truthiness =="
p m.respond_to?(:pub)
p m.respond_to?(:priv)           # private excluded by default
p m.respond_to?(:prot)           # protected excluded by default
p m.respond_to?(:pub, true)
p m.respond_to?(:priv, true)
p m.respond_to?(:prot, true)
p m.respond_to?(:priv, :truthy)  # truthy non-Bool counts as include-all
p m.respond_to?(:prot, "yes")
p m.respond_to?(:priv, nil)      # nil is falsy
p m.respond_to?(:priv, false)
p m.respond_to?("pub")
p m.respond_to?("priv")
p m.respond_to?(:nope)

puts "== Class receivers =="
class KlassM
  def self.cpub; :cpub; end
  class << self
    private def cpriv; :cpriv; end
  end
end
p KlassM.public_send(:cpub)
p KlassM.send(:cpriv)
begin; KlassM.public_send(:cpriv); rescue NoMethodError => e; puts "cpriv: #{e.message}"; end

puts "== visibility changed at runtime =="
class Mutable
  def flip_me; :flipped; end
end
mu = Mutable.new
p mu.public_send(:flip_me)
class Mutable
  private :flip_me
end
begin; mu.public_send(:flip_me); rescue NoMethodError => e; puts "flipped: #{e.message}"; end
p mu.send(:flip_me)
p mu.respond_to?(:flip_me)
p mu.respond_to?(:flip_me, true)

puts "== subclass inherits visibility =="
class Base
  private def base_priv; :bp; end
  def base_pub; :bpub; end
end
class Sub < Base; end
s = Sub.new
p s.public_send(:base_pub)
begin; s.public_send(:base_priv); rescue NoMethodError => e; puts "sub-priv: #{e.message}"; end
p s.send(:base_priv)
