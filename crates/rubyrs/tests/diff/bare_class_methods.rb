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
## respond_to set. This fixture pins the **non-mutating subset**
## of that whitelist plus the special `allocate` case (bridge
## re-entry through the dedicated arm with stricter fences):
##   - identity: `name` / `to_s` / `inspect`
##   - hierarchy: `superclass` / `ancestors` / `include?`
##   - introspection: `method_defined?` / `instance_method` /
##     `instance_methods` / `public_instance_methods` /
##     `private_instance_methods` / `protected_instance_methods`
##   - constants: `constants`
##   - meta: `singleton_class`
##   - allocator: `allocate` (bare-form + user-singleton-override)
##   - construction: `new`
## Whitelisted names NOT covered here are deliberate skips:
## `undef_method` and the constant-management quartet (`autoload`
## / `private_constant` / `public_constant` / `deprecate_constant`)
## are mutating — they'd either change subsequent assertions in
## the same body or have side-effects the byte-for-byte diff
## harness doesn't tolerate cleanly. Their bare-call dispatch is
## still pinned via the lockstep respond_to surface; the bridge
## forwarding is exercised whenever the receiver-form arm is hit.

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

  ## `new` — bare form, the canonical bridge case (msgpack-
  ## ruby's timestamp.rb does `def self.from_msgpack_ext(...);
  ## new(...); end`). Class has an `initialize` via inheritance
  ## from Object's default, so `new` with no args works.
  puts "bare-new-class=#{new.class.name}"
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

## Module fence on bare `allocate`: a bare `allocate` from
## inside a `module Foo; ... end` body must fall through the
## bridge (lookup.rs respond_to says false on Modules) and
## reach the toplevel/method_missing tail. CRuby raises
## NameError ("undefined local variable or method"); rubyrs
## raises NoMethodError ("undefined method for Class"). In
## CRuby NoMethodError is a NameError subclass, so `rescue
## NameError` would catch both — but rubyrs models them as
## sibling subclasses of StandardError. We explicitly pin
## BOTH classes via a two-arm rescue: a regression that drops
## the fence would route bare allocate to the dedicated arm's
## TypeError, which neither arm catches, so the fixture would
## fail loudly instead of silently accepting `!msg.empty?`
## for any error class. PR #196 code-review #2.
## Block-form bare calls — `do_call_block`'s parallel bridge
## (PR #196 code-review #3). CRuby silently discards the block
## for whitelisted Class methods like `ancestors` / `superclass`
## / `name`; verified locally that the block does NOT run. Pre-
## existing gap before this PR: do_call_block had no Class
## bridge, so block-form bare calls inside a class body raised
## NoMethodError even when the blockless form worked.
class BlockBareBar < Foo
  block_marker = "not-run"
  result = ancestors { block_marker = "ran" }
  puts "block-bare-ancestors-prefix=#{result.map(&:to_s).take(3).inspect}"
  puts "block-bare-block-discarded=#{block_marker.inspect}"

  name_result = name { block_marker = "ran-name" }
  puts "block-bare-name=#{name_result.inspect}"
  puts "block-bare-block-discarded-name=#{block_marker.inspect}"
end

module ModAllocFence
  begin
    allocate
    puts "module-allocate=DID-NOT-RAISE"
  rescue NoMethodError => e
    puts "module-allocate=fenced-family:#{!e.message.empty?}"
  rescue NameError => e
    puts "module-allocate=fenced-family:#{!e.message.empty?}"
  end
end
