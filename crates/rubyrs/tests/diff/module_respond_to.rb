# `respond_to?` on a Class / Module receiver reports the public Module and
# Kernel methods every class inherits, and keeps Module's private
# visibility surface behind `include_all`. Previously `Klass.respond_to?(:===)`
# was false, so ActiveSupport 8.1's `rescue_from(SomeError)` raised
# ArgumentError; and `module_function` answered true without include_all
# (and on classes, where CRuby undefines it).

class E < StandardError; end
module Mo; end

PUB = %i[=== instance_of? is_a? kind_of? public_send dup clone extend include prepend
  attr_accessor attr_reader attr_writer attr alias_method
  class_variable_get class_variable_set class_variables class_variable_defined?
  module_exec class_exec const_source_location
  public_method_defined? private_method_defined? protected_method_defined?]
PRIV = %i[private public protected module_function remove_const]

[E, Mo, Class, Object, Comparable, Module.new, Class.new].each do |r|
  label = r.name || r.class.name
  puts "#{label} pub: #{PUB.reject { |m| r.respond_to?(m) }.inspect}"
  puts "#{label} priv hidden: #{PRIV.select { |m| r.respond_to?(m) }.inspect}"
  puts "#{label} priv all: #{PRIV.select { |m| r.respond_to?(m, true) }.inspect}"
end

# Each reported method does dispatch on a class receiver.
p E === E.new, E === 1, Mo === 1
p E.instance_of?(Class), Mo.is_a?(Module), E.kind_of?(Class)
p E.public_send(:name)
p E.dup.class, Mo.clone.class

# The AS Rescuable guard shape.
def rescuable?(k) = k.is_a?(Module) && k.respond_to?(:===)
p rescuable?(E), rescuable?(Mo), rescuable?("E")

# A user class method still counts; a private one only with include_all.
class E
  def self.pub_cm = 1
  def self.priv_cm = 2
  private_class_method :priv_cm
end
p E.respond_to?(:pub_cm), E.respond_to?(:priv_cm), E.respond_to?(:priv_cm, true)
