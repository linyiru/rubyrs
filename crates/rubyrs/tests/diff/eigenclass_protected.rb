## S3 item (e): bare `protected` (and `private`, and the args forms)
## in eigenclass bodies are ENFORCED on class-method dispatch.
## CRuby semantics (probed 3.4.8):
##   - a public class method may call a protected sibling on an
##     explicit receiver;
##   - an external (toplevel / unrelated) caller raises
##     "protected method 'x' called for class X";
##   - a SUBCLASS's class method MAY call it cross-receiver (the
##     subclass object IS an instance of the base's metaclass) —
##     rouge's `Lexer.register` pattern;
##   - an INSTANCE of the class may NOT (an instance is never an
##     instance of the metaclass);
##   - respond_to? is false; protected_instance_methods(false) on the
##     singleton class lists it.

class Widget
  class << self
    def pub
      "pub:" + prot_helper
    end

    protected

    def prot_helper
      "prot"
    end
  end
end

puts "pub=#{Widget.pub}"

begin
  Widget.prot_helper
  puts "external=NOT-ENFORCED"
rescue NoMethodError => e
  puts "external=#{e.message}"
end

class Sub < Widget
  def self.try_cross
    Widget.prot_helper
  rescue NoMethodError
    "cross NoMethodError"
  end
end
puts "subclass_cross=#{Sub.try_cross}"

class Widget
  def inst_try
    Widget.prot_helper
  rescue NoMethodError
    "inst NoMethodError"
  end
end
puts "instance_caller=#{Widget.new.inst_try}"

sc = Widget.singleton_class
puts "protected_list=#{sc.protected_instance_methods(false).inspect}"
puts "public_lists_it=#{sc.public_instance_methods(false).include?(:prot_helper)}"
puts "respond=#{Widget.respond_to?(:prot_helper)}"
puts "respond_all=#{Widget.respond_to?(:prot_helper, true)}"

## send bypasses (CRuby).
puts "send=#{Widget.send(:prot_helper)}"

## public_send stays strict.
begin
  Widget.public_send(:prot_helper)
  puts "public_send=NOT-ENFORCED"
rescue NoMethodError
  puts "public_send=NoMethodError"
end

## Args form `protected :name`.
class Gadget
  class << self
    def g1
      "g1:" + g2
    end
    def g2
      "g2"
    end
    protected :g2
  end
end
puts "g1=#{Gadget.g1}"
begin
  Gadget.g2
  puts "g2=NOT-ENFORCED"
rescue NoMethodError
  puts "g2=NoMethodError"
end

## Bare `private` in an eigenclass body (was a silent no-op on the
## desugar path — only `private def` worked).
class Gizmo
  class << self
    def zpub
      zpriv
    end

    private

    def zpriv
      "zpriv"
    end
  end
end
puts "zpub=#{Gizmo.zpub}"
begin
  Gizmo.zpriv
  puts "zpriv=NOT-ENFORCED"
rescue NoMethodError => e
  puts "zpriv=#{e.message}"
end

## Args form `private :name` on a user-defined class method (the
## Liquid `class << self; private :new` shape targets the BUILTIN
## constructor, which stays a documented no-op — see SUBSET.md's
## "privatising builtin class methods" entry; user-defined records
## are enforced).
class Widgetry
  class << self
    def build
      construct
    end
    def construct
      "built"
    end
    private :construct
  end
end
puts "build=#{Widgetry.build}"
begin
  Widgetry.construct
  puts "construct=NOT-ENFORCED"
rescue NoMethodError => e
  puts "construct=#{e.class}"
end

## `public` re-export in the same body.
class Reexport
  class << self
    private
    def was_private
      "now-public"
    end
    public :was_private
  end
end
puts "reexport=#{Reexport.was_private}"

## Module spelling: a module's own class method may self-call a
## protected sibling via an explicit receiver.
module Registry
  class << self
    def go
      Registry.prot2
    end

    protected

    def prot2
      "p2"
    end
  end
end
puts "module_self=#{Registry.go}"
begin
  Registry.prot2
  puts "module_external=NOT-ENFORCED"
rescue NoMethodError
  puts "module_external=NoMethodError"
end

## ---------------------------------------------------------------
## `extend`-acquired protected class methods (the AS::Callbacks
## set/get_callbacks shape that broke ActiveModel boot). CRuby's
## kin rule here is the FULL `caller.is_a?(CM)` — the caller's
## SINGLETON chain counts (`Person.extend(CM)` makes
## `Person.is_a?(CM)` true), NOT just the caller's class chain.
## Probed CRuby 3.4.8 + 3.4.1.
module KinCM
  def kprot
    "kprot:#{self}"
  end
  protected :kprot
end

class KinHost
  extend KinCM
  def self.call_own
    KinHost.kprot
  rescue NoMethodError
    "NoMethodError"
  end
end

class KinSub < KinHost
  def self.call_cross
    KinHost.kprot
  rescue NoMethodError
    "NoMethodError"
  end

  def self.call_self
    KinSub.kprot
  rescue NoMethodError
    "NoMethodError"
  end
end

class KinUnrelated
  def self.call_other
    KinHost.kprot
  rescue NoMethodError => e
    "NoMethodError: #{e.message}"
  end
end

puts "ext_is_a=#{KinHost.is_a?(KinCM)}"
puts "ext_own=#{KinHost.call_own}"
puts "ext_sub_cross=#{KinSub.call_cross}"
puts "ext_sub_self=#{KinSub.call_self}"
puts "ext_unrelated=#{KinUnrelated.call_other}"
begin
  KinHost.kprot
  puts "ext_toplevel=NOT-ENFORCED"
rescue NoMethodError => e
  puts "ext_toplevel=#{e.message}"
end

## An INSTANCE of the extender may NOT (its singleton chain does
## not include KinCM)…
class KinHost
  def inst_try
    KinHost.kprot
  rescue NoMethodError
    "NoMethodError"
  end
end
puts "ext_instance=#{KinHost.new.inst_try}"

## …but an object that itself `extend`s the module MAY (full is_a?
## honours the object's eigenclass), and so may an instance of a
## class that INCLUDEs it.
class KinProbe
  def go
    KinHost.kprot
  rescue NoMethodError
    "NoMethodError"
  end
end
kp = KinProbe.new
kp.extend(KinCM)
puts "ext_extended_obj=#{kp.go}"

class KinIncluder
  include KinCM
  def go
    KinHost.kprot
  rescue NoMethodError
    "NoMethodError"
  end
end
puts "ext_includer_instance=#{KinIncluder.new.go}"

## respond_to? / send / public_send parity for the extend shape.
puts "ext_respond=#{KinHost.respond_to?(:kprot)}"
puts "ext_respond_all=#{KinHost.respond_to?(:kprot, true)}"
puts "ext_send=#{KinHost.send(:kprot)}"
begin
  KinHost.public_send(:kprot)
  puts "ext_public_send=NOT-ENFORCED"
rescue NoMethodError
  puts "ext_public_send=NoMethodError"
end

## `singleton_class.include` spelling of the same acquisition.
module KinCM2
  def kprot2
    "p2:#{self}"
  end
  protected :kprot2
end
class KinViaInclude
  singleton_class.include KinCM2
  def self.own
    KinViaInclude.kprot2
  rescue NoMethodError
    "NoMethodError"
  end
end
puts "ext_via_include=#{KinViaInclude.own}"

## Pinned NEGATIVE: a module's OWN eigenclass-protected class
## method keeps the strict metaclass rule — `include`-ing or
## `extend`-ing the module does NOT unlock it (the defining scope
## is #<Class:KinReg>, not KinReg).
module KinReg
  class << self
    def go
      KinReg.kp2
    end

    protected

    def kp2
      "kp2"
    end
  end
end
class KinUsesReg
  include KinReg
  def try
    KinReg.kp2
  rescue NoMethodError
    "NoMethodError"
  end
end
class KinExtReg
  extend KinReg
  def self.try
    KinReg.kp2
  rescue NoMethodError
    "NoMethodError"
  end
end
puts "reg_self=#{KinReg.go}"
puts "reg_includer_instance=#{KinUsesReg.new.try}"
puts "reg_extender_class=#{KinExtReg.try}"

## Instance-receiver twin: a protected INSTANCE method defined in a
## module is callable by a CLASS caller that `extend`s the module
## (caller.is_a?(M) via its singleton chain), denied for a plain
## class caller.
module KinIM
  def iprot
    "iprot!"
  end
  protected :iprot
end
class KinIA
  include KinIM
  def sib(other)
    other.iprot
  end
end
class KinIB
  extend KinIM
  def self.try
    KinIA.new.iprot
  rescue NoMethodError
    "NoMethodError"
  end
end
class KinIC
  def self.try
    KinIA.new.iprot
  rescue NoMethodError
    "NoMethodError"
  end
end
puts "inst_class_extender=#{KinIB.try}"
puts "inst_plain_class=#{KinIC.try}"
puts "inst_includer_sibling=#{KinIA.new.sib(KinIA.new)}"
