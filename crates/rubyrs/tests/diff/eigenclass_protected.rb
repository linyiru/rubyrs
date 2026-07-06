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
