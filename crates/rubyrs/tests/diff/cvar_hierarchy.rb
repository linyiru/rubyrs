# Class variables are shared across the class hierarchy: a `@@x`
# defined in a parent is the SAME variable read/written from a
# subclass (and from inherited class methods). Discovery: P3 Jekyll
# spike — kramdown's `Kramdown::Parser::Kramdown` sets `@@parsers = {}`
# and its `SmartyPants` subclass calls the inherited `define_parser`,
# which reads `@@parsers`.
class Base
  @@registry = {}
  def self.define(name); @@registry[name] = true; end
  def self.has?(name); @@registry.key?(name); end
  def self.count; @@registry.size; end
end

class Child < Base
  define(:from_child)     # inherited class method reaches Base's @@registry
end

class GrandChild < Child
  define(:from_grandchild)
end

Base.define(:from_base)

p Base.has?(:from_child)       # shared: subclass write visible on parent
p Base.has?(:from_grandchild)
p Child.has?(:from_base)       # shared: parent write visible on subclass
p GrandChild.has?(:from_base)
p Base.count

# write from an INSTANCE method also targets the shared variable
class Widget
  @@instances = 0
  def initialize; @@instances += 1; end
  def self.instances; @@instances; end
end
class Gadget < Widget; end
Widget.new
Gadget.new
Gadget.new
p Widget.instances    # 3 — shared across Widget + Gadget
p Gadget.instances
