## S3 item (c): `def self.x` inside an eigenclass body defines a
## method on the eigenclass's OWN eigenclass — callable as
## `X.singleton_class.x`, NOT as `X.x`. The desugar used to discard
## the inner `self` receiver (silently minting a plain class method);
## such bodies now route to the real eigenclass-body path, where
## Op::DefSingletonMethod sees the shell on the class_stack and
## installs into the shell's singleton_methods.

class Widget
  class << self
    def self.meta
      "meta-method"
    end
    def normal
      "normal-classmethod"
    end
  end
end

puts "normal=#{Widget.normal}"

sc = Widget.singleton_class
puts "sc_meta=#{sc.meta}"

begin
  Widget.meta
  puts "widget_meta=WRONGLY-DEFINED"
rescue NoMethodError
  puts "widget_meta=NoMethodError"
end

## Reflection: meta is a singleton method OF the singleton class.
puts "sc_singleton_methods=#{sc.singleton_methods(false).inspect}"
puts "sc_meta_respond=#{sc.respond_to?(:meta)}"
puts "widget_meta_respond=#{Widget.respond_to?(:meta)}"

## `class << Const` spelling.
class Gadget; end
class << Gadget
  def self.gmeta
    "gmeta"
  end
end
puts "gmeta=#{Gadget.singleton_class.gmeta}"
begin
  Gadget.gmeta
  puts "gadget_gmeta=WRONGLY-DEFINED"
rescue NoMethodError
  puts "gadget_gmeta=NoMethodError"
end
