# Introspecting an eigenclass shell — `Klass.singleton_class.instance_method
# (:m)` / `.method_defined?(:m)` / `.instance_methods(false)` — must see
# class-level singleton methods (`def self.m`), which install into the real
# class's singleton table, not the shell's own. Surfaced by sorbet-runtime's
# `singleton_class.instance_method(name)` over a `def self.included`.
class Foo
  def self.bar; 42; end
  def self.baz; :b; end
  def self.hidden; :h; end
  private_class_method :hidden
end
sc = Foo.singleton_class
p sc.instance_method(:bar).class                 # UnboundMethod
p sc.instance_method(:bar).name                  # :bar
p sc.method_defined?(:bar)                        # true
p sc.method_defined?(:nope)                       # false
names = sc.instance_methods(false)
p names.include?(:bar)                            # true
p names.include?(:baz)                            # true
p names.include?(:hidden)                         # false (private)
# inherited singleton method via superclass shell
class Sub < Foo; end
p Sub.singleton_class.method_defined?(:bar)       # true
p Sub.singleton_class.instance_method(:baz).class # UnboundMethod
# unknown name raises NameError
begin
  sc.instance_method(:missing)
rescue NameError
  puts "NameError"
end
