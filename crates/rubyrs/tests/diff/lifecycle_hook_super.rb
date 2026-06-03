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

## Shape 3: super-chain. A3 → A2 → A1 → ... none has a real
## inherited override, all use the no-op terminator.
class A3
  def self.inherited(sub); super; end
end
class B3 < A3; end
puts "shape3-class=#{B3.superclass.inspect}"

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

## Shape 5: skipped — would have tested `extended` hook via
## `Object#extend(M)`, but rubyrs doesn't fire the
## `Module#extended` callback yet (orthogonal Layer #14-style
## hook gap, separate PR). The `super`-no-op intercept added
## here DOES list `extended` in the whitelist, so once the
## firing path lands the super-call from inside an `extended`
## override will work automatically.

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
