## `alias new! new` inside `class << self` body — aliasing a
## built-in Class method (`Class#new`) as a singleton method.
## Closes TRY_RUNS pass-9.7 layer #19 — sinatra/base.rb:1659
## does exactly this:
##   class << self
##     alias new! new unless method_defined? :new!
##   end
##
## Before this fix, Op::AliasSingletonMethod's user-singleton
## lookup returned None (`:new` isn't a user-defined singleton
## method — it's `Class#new`, the built-in instantiator) and
## the alias raised NameError at class-body load time. The new
## fallback synthesises a forwarder Method whose body
## dispatches the original name (here `:new`) on `self`,
## reusing the same `synth_primitive_forwarder` shape that
## Op::AliasMethod already uses for the instance-method
## variant.
##
## The forwarder is class-agnostic — when `Foo.new!(args)` is
## invoked later, the body runs `self.new(*args)` which
## routes through Class#new's existing arm and instantiates
## the receiver class.

class Box
  attr_reader :ts
  def initialize(ts)
    @ts = ts
  end

  class << self
    ## Alias the built-in `new` as `new!`. CRuby and rubyrs
    ## now agree: `new!` instantiates Box with the same arg.
    alias new! new
  end
end

b = Box.new!(42)
puts "class=#{b.class.name}"
puts "ts=#{b.ts}"

## Aliasing other built-in Class methods follows the same path:
## any name advertised by lookup.rs's Value::Class respond_to
## whitelist (`name`, `to_s`, `ancestors`, ...) is reachable.
class Bar
  class << self
    alias my_name name
    alias my_to_s to_s
    alias my_ancestors_prefix ancestors
  end
end

puts "alias-name=#{Bar.my_name.inspect}"
puts "alias-to_s=#{Bar.my_to_s.inspect}"
## Ancestor chain — only assert the user-class prefix; CRuby
## walks all the way to BasicObject while rubyrs's chain
## bottoms out earlier (separate KNOWN GAP, see
## class_allocate.rb).
puts "alias-ancestors-prefix=#{Bar.my_ancestors_prefix.map(&:to_s).take(1).inspect}"

## Missing-source case (e.g. `alias bad nope_no_such_method`)
## still raises NameError at class-body load time — the
## fallback only fires when `responds_to(Class, old_id)` is
## true (i.e., the name is in the lookup.rs Class respond_to
## whitelist). Not asserted here because the raise fires
## during class-body load and aborts the script; the assertion
## would need an eval/class_eval harness that the byte-for-byte
## diff harness doesn't compose with cleanly. Verified via a
## standalone repro outside the diff suite.
