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
## rubyrs's `no_recv` bare-call arm was only forwarding a
## five-name whitelist (`new`, `name`, `method_defined?`,
## `instance_method`, `undef_method`); other built-in Class
## methods like `superclass` / `ancestors` / `include?` /
## `singleton_class` / `to_s` / `inspect` were missing despite
## being already-listed by lookup.rs's respond_to set.

module Mod; def from_mod; "M"; end; end
class Foo
  include Mod
end
class Bar < Foo
  ## The layer #8 minimal repro.
  superclass.class_eval do
    def hi; "hi-from-Foo-via-superclass.class_eval"; end
  end

  ## Pin every bare-callable Class method that lookup.rs's
  ## respond_to whitelist advertises.
  puts "bare-name=#{name.inspect}"
  puts "bare-to_s=#{to_s.inspect}"
  puts "bare-inspect=#{inspect.inspect}"
  puts "bare-superclass=#{superclass.inspect}"
  ## Ancestor chain — only assert the user-class prefix; CRuby
  ## walks all the way to BasicObject while rubyrs's chain bottoms
  ## out earlier (separate KNOWN GAP). The bare-call resolution
  ## itself is what this fixture is pinning.
  puts "bare-ancestors-prefix=#{ancestors.map(&:to_s).take(3).inspect}"
  puts "bare-include-mod=#{include?(Mod)}"
  puts "bare-instance-methods-has-hi=#{instance_methods.include?(:hi)}"
  puts "bare-singleton-class-is-class=#{singleton_class.is_a?(Class)}"
end

puts "after-class-body=#{Bar.new.hi}"
puts "after-class-body-from-mod=#{Bar.new.from_mod}"
