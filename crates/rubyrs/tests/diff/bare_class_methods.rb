## Bare calls to built-in `Class` methods from inside a class body
## must resolve to the implicit receiver (the class being defined),
## not fall through to toplevel and raise NoMethodError. Surfaced
## by TRY_RUNS pass 8 layer #8: sinatra/base.rb does
## `class Bar < Foo; superclass.class_eval { ... }; end`, which
## raised `NoMethodError: undefined method 'superclass' for Class`
## before the dispatch bridge whitelist was expanded.
##
## CRuby parity: every name in this fixture works as a bare call
## inside `class Bar < Foo; ... end` because `self` IS the class.
## The bridge whitelist mirrors lookup.rs's `Value::Class(_)`
## respond_to set (vm/lookup.rs:590-624). This fixture pins the
## **whole set** including `allocate` (which routes through a
## dedicated arm with stricter fences via bridge re-entry).

module Mod; def from_mod; "M"; end; end
class Foo
  include Mod
end
class Bar < Foo
  ## The layer #8 minimal repro.
  superclass.class_eval do
    def hi; "hi-from-Foo-via-superclass.class_eval"; end
  end

  ## Core identity / coercion.
  puts "bare-name=#{name.inspect}"
  puts "bare-to_s=#{to_s.inspect}"
  puts "bare-inspect=#{inspect.inspect}"

  ## Hierarchy.
  puts "bare-superclass=#{superclass.inspect}"
  ## Ancestor chain — only assert the user-class prefix; CRuby
  ## walks all the way to BasicObject while rubyrs's chain bottoms
  ## out earlier (separate KNOWN GAP). The bare-call resolution
  ## itself is what this fixture is pinning.
  puts "bare-ancestors-prefix=#{ancestors.map(&:to_s).take(3).inspect}"
  puts "bare-include-mod=#{include?(Mod)}"

  ## Method introspection — bare forms.
  puts "bare-method_defined?-hi=#{method_defined?(:hi)}"
  puts "bare-method_defined?-nope=#{method_defined?(:nope_no_such_method)}"
  puts "bare-instance_method-class=#{instance_method(:hi).class.name}"
  ## `instance_methods` chains can be huge with all inherited
  ## methods — `.include?(:hi)` gives a stable assertion that
  ## doesn't depend on whether rubyrs's ancestor walk reaches
  ## Kernel/BasicObject.
  puts "bare-instance_methods-has-hi=#{instance_methods.include?(:hi)}"
  puts "bare-public_instance_methods-has-hi=#{public_instance_methods.include?(:hi)}"
  puts "bare-private_instance_methods-class=#{private_instance_methods(false).class.name}"
  puts "bare-protected_instance_methods-class=#{protected_instance_methods(false).class.name}"

  ## Constants — bare form. Empty for this class.
  puts "bare-constants=#{constants.inspect}"

  ## Singleton class — bare form. Don't compare values (they
  ## differ across runs); just confirm it's a Class.
  puts "bare-singleton-class-is-class=#{singleton_class.is_a?(Class)}"

  ## `allocate` — bare form routes through the dedicated arm
  ## (with all its fences intact) via bridge re-entry. CRuby
  ## allows it and produces a bare instance whose class is the
  ## current class.
  puts "bare-allocate-class=#{allocate.class.name}"
end

## Bare `allocate` also honors `def self.allocate` overrides
## (PR #181 / code-review #1 added the singleton check). Pin it
## here from a subclass body so the bridge re-entry is exercised.
class FooWithCustomAlloc
  def self.allocate; "user-allocate-via-bare"; end
end
class BarFromCustom < FooWithCustomAlloc
  puts "bare-allocate-with-override=#{allocate.inspect}"
end

puts "after-class-body=#{Bar.new.hi}"
puts "after-class-body-from-mod=#{Bar.new.from_mod}"
