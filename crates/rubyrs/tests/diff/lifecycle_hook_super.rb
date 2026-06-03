## `super` from lifecycle-hook bodies (`inherited` / `included` /
## `extended` / `method_added` / etc) resolves to a default
## no-op in CRuby — `Class` ships real no-op implementations of
## these hooks so an overriding body can call `super` without
## a NoMethodError. PR #337 implemented hook firing but
## bypassed the method install, so `super` walked Sinatra::Base
## → Object → BasicObject without finding `inherited` and
## raised NoMethodError. Sinatra-4's `Sinatra::Base.inherited`
## calls `super` on every subclass; without the no-op handling
## the entire load chain stops at base.rb:1894.
##
## (TRY_RUNS pass-15 layer #20.)

## Shape 1: bare super inside `inherited`. CRuby treats this
## as a call into the no-op default, returning nil.
$shape1_ret = :sentinel
class A1
  def self.inherited(sub)
    $shape1_ret = super
  end
end
class B1 < A1; end
puts "shape1-super-ret=#{$shape1_ret.inspect}"

## Shape 2: explicit args super inside `inherited`. The
## default no-op ignores the args, returns nil.
class A2
  def self.inherited(sub)
    puts "A2.inherited body fired with #{sub}"
    super(sub)
    puts "A2.inherited after super"
  end
end
class B2 < A2; end

## Shape 3: multi-level inherited chain — `Leaf < LowA < MidA
## < TopA`, with `LowA`, `MidA`, `TopA` each overriding
## `inherited` and calling super. When `class Leaf < LowA` is
## defined Ruby fires `inherited` starting at the direct
## superclass (`LowA`) and each level's `super` walks one step
## upward (LowA → MidA → TopA → Class no-op default).
## Code-review #363 round 1 caught the original Shape 3 only
## exercising a single class without an actual chain; round 2
## tightened the comment to describe the walk direction.
$shape3_log = []
class TopA
  def self.inherited(sub)
    $shape3_log << "TopA.inherited(#{sub})"
    super
  end
end
class MidA < TopA
  def self.inherited(sub)
    $shape3_log << "MidA.inherited(#{sub})"
    super
  end
end
class LowA < MidA
  def self.inherited(sub)
    $shape3_log << "LowA.inherited(#{sub})"
    super
  end
end
class Leaf < LowA; end
puts "shape3-chain=#{$shape3_log.inspect}"
puts "shape3-leaf-ancestors=#{Leaf.ancestors.take(4).inspect}"

## Shape 4: included hook on Module — same pattern. Bare
## super hits the Module#included default no-op.
module M4
  def self.included(klass)
    $shape4_log = "included:#{klass}"
    super
  end
end
class C4
  include M4
end
puts "shape4=#{$shape4_log.inspect}"

## Shape 4p: `prepended` hook on Module — fires the same code
## path as `included` (vm/dispatch.rs's fire_inclusion_hooks
## with hook_name="prepended") with the same Class/Module
## receiver, so its `super` must also reach the Module no-op
## default. /code-review caught the omission — Layer #20's
## first cut whitelisted 9 hook names but missed `prepended`,
## reintroducing exactly the bug shape this PR was meant to
## close, just on the prepend side.
module M4p
  def self.prepended(klass)
    $shape4p_log = "prepended:#{klass}"
    super
  end
end
class C4p
  prepend M4p
end
puts "shape4p=#{$shape4p_log.inspect}"

## Shape 5: skipped — would have tested `extended` hook via
## `Object#extend(M)`, but rubyrs doesn't fire the
## `Module#extended` callback yet (orthogonal Layer #14-style
## hook gap, separate PR). The `super`-no-op intercept added
## here DOES list `extended` in the whitelist, so once the
## firing path lands the super-call from inside an `extended`
## override will work automatically.

## Shape 6a: regression — `super` called outside of any
## method body (toplevel / class-body) must STILL raise even
## when the surrounding context's method name would have been
## a lifecycle hook. The intercept discriminates on the
## error-message shape (\"no superclass method\" vs \"called
## outside of method\"), so this sibling NoMethodError variant
## propagates as a hard error. Code-review #363 round 1
## caught the over-broad match.
err = begin
  # Use eval to invoke super from toplevel — no method frame
  # means no defining_class, so super_lookup hits the
  # "called outside of method" path. (Skipping `binding` arg
  # because rubyrs doesn't model Kernel#binding yet — eval
  # uses the toplevel context by default.)
  eval("super")
  "no-raise"
rescue NoMethodError => e
  e.message.include?("super called outside of method") ? "outside-method-trapped" : "other"
rescue => e
  "other:#{e.class}"
end
puts "shape6a=#{err}"

## Shape 6: regression — `super` for a NON-lifecycle method
## name that genuinely has no parent must STILL raise
## NoMethodError. The intercept only fires for the known
## hook names.
err = begin
  class P6
    def self.unrelated_method
      super
    end
  end
  P6.unrelated_method
  "no-raise"
rescue NoMethodError => e
  e.message.include?("super: no superclass method") ? "no-method-trapped" : "other"
end
puts "shape6=#{err}"

## Shape 7: regression — an ordinary user object whose author
## happened to name an instance method one of the lifecycle
## names (`included`, `inherited`, etc.) must STILL get
## NoMethodError when its `super` call walks off the chain.
## The intercept is scoped to Class/Module receivers; for a
## plain object the no-op default doesn't apply.
## Code-review #363 round 2.
err = begin
  class P7
    def included(arg)  # instance method, NOT Module#included
      super
    end
  end
  P7.new.included(:dummy)
  "no-raise"
rescue NoMethodError => e
  e.message.include?("super: no superclass method") ? "no-method-trapped" : "other"
end
puts "shape7=#{err}"
