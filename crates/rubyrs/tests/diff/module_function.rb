## `Module#module_function` — switches subsequent `def`s to
## module-function mode AND/OR retroactively converts named
## methods. CRuby semantics:
##   - Bare form: subsequent instance methods become private
##     AND get copied to the module's singleton class (callable
##     as `M.foo`).
##   - Symbol/String args: for each named already-defined
##     method, copy from instance methods to singleton class
##     and mark the instance copy private.
##
## Discovery context: rack-3.1.10/lib/rack/utils.rb uses both
## forms during sinatra-4's load chain. (TRY_RUNS pass-10
## layer #12.)
##
## Tier-1 partial-implementation: bare form switches
## visibility to Private (so the instance copy is private) but
## does NOT auto-mirror subsequent defs to the singleton class
## — `Rack::Utils.escape(...)` would fail at runtime. The
## explicit Symbol-arg form DOES do the proper dual-install,
## which is what most gems use.

## Shape 1: bare `module_function` switches visibility to
## Private. Subsequent `def`s become private instance methods
## (but not auto-mirrored — Tier-1 divergence).
module M1
  module_function
  def helper; "h"; end
end
puts "private?=#{M1.private_instance_methods.include?(:helper)}"

## Shape 2: explicit `module_function :sym` — copies the
## already-defined `:foo` to the module's singleton (so
## `M.foo` dispatches) AND marks the instance copy private.
module M2
  def foo; "via-foo"; end
  def bar; "via-bar"; end
  module_function :foo
end
puts "M2.foo=#{M2.foo}"
puts "foo-private?=#{M2.private_instance_methods.include?(:foo)}"
puts "bar-still-public?=#{M2.public_instance_methods.include?(:bar)}"
puts "M2-singleton-foo?=#{M2.singleton_methods.include?(:foo)}"

## Shape 3: multiple Symbol args.
module M3
  def a; 1; end
  def b; 2; end
  def c; 3; end
  module_function :a, :c
end
puts "M3.a=#{M3.a}"
puts "M3.c=#{M3.c}"
puts "a-private?=#{M3.private_instance_methods.include?(:a)}"
puts "b-public?=#{M3.public_instance_methods.include?(:b)}"
puts "c-private?=#{M3.private_instance_methods.include?(:c)}"

## Shape 4: String args also work (CRuby semantics — strings
## coerce to symbols).
module M4
  def hello; "hi"; end
  module_function "hello"
end
puts "M4.hello=#{M4.hello}"

## Shape 5: `Object.new.send(:module_function)` — `send`
## bypasses private-method checks, so the resulting error is
## an UNDEFINED-method NoMethodError (Object instances don't
## define module_function at all). Post-#324 round 2 rubyrs
## falls through (the intercept arm only fires for
## `Value::Class` receivers), so the runtime undefined-method
## NoMethodError surfaces naturally. Substring-tolerant
## check on the message accepts both interpreters' wording.
err = begin
  Object.new.send(:module_function)
  "no-raise"
rescue NoMethodError => e
  e.message.include?("module_function") ? "rejected" : "other-NoMethodError"
end
puts "main-context=#{err}"
