## `Class#inherited` hook — CRuby fires `Parent.inherited(Sub)`
## automatically right after `class Sub < Parent` creates the
## subclass object but BEFORE the body runs. Before this layer's
## fix, rubyrs never fired the callback, so frameworks like
## Sinatra (whose `inherited` hook calls `subclass.reset!` to
## initialize `@routes` etc) crashed at the first DSL call on a
## fresh subclass.
##
## Discovery: TRY_RUNS pass-12 — sinatra/base.rb:1781
## `(@routes[verb] ||= [])` raised NoMethodError because
## `Sinatra::Base.inherited` never fired on `class App < Sinatra::Base`.
## (Layer #14.)

## Shape 1: bare fire — callback receives the subclass.
$shape1_log = []
class S1Parent
  def self.inherited(sub)
    $shape1_log << "got:#{sub}"
  end
end
class S1Child < S1Parent
end
puts "shape1=#{$shape1_log.inspect}"

## Shape 2: callback runs BEFORE the body executes. Set an ivar
## from the callback; the body must see it.
class S2Parent
  def self.inherited(sub)
    sub.instance_variable_set(:@from_hook, :hello)
  end
end
class S2Child < S2Parent
  $shape2_body_seen = self.instance_variable_get(:@from_hook)
end
puts "shape2-pre-body=#{$shape2_body_seen.inspect}"

## Shape 3: reopen does NOT re-fire (CRuby fires only on first
## definition).
$shape3_count = 0
class S3Parent
  def self.inherited(sub); $shape3_count += 1; end
end
class S3Child < S3Parent; end
class S3Child; end          # reopen — must NOT trigger
class S3Child < S3Parent; end  # second `< Parent` — also no-op
puts "shape3-count=#{$shape3_count}"

## Shape 4: no callback override — silent no-op (default behavior).
class S4Parent; end
class S4Child < S4Parent; end
puts "shape4-ok"

## Shape 5: callback can mutate the subclass (the sinatra pattern).
## Subclass.reset!-style initialization works.
class S5Parent
  def self.inherited(sub)
    sub.instance_variable_set(:@routes, {})
    sub.instance_variable_set(:@count, 0)
  end
end
class S5Child < S5Parent
  @count += 1
  @routes[:get] = ['/']
end
puts "shape5-routes=#{S5Child.instance_variable_get(:@routes).inspect}"
puts "shape5-count=#{S5Child.instance_variable_get(:@count)}"

## Shape 6: module — does NOT fire `inherited` (modules don't
## have a superclass / inheritance).
$shape6_fired = false
class S6Parent
  def self.inherited(sub); $shape6_fired = true; end
end
module S6Mod; end          # not a subclass, no callback
puts "shape6-module-fired=#{$shape6_fired}"

## Shape 7: inherited resolution walks the SINGLETON ancestor
## chain — `def self.inherited` defined on a grandparent fires
## for grandchildren too. This is what `lookup_class_singleton_
## method` already supports; pins the singleton-walk path.
class S7Grandparent
  def self.inherited(sub); puts "gp.inh: #{sub}"; end
end
class S7Parent < S7Grandparent; end   # gp.inh fires for parent
class S7Child < S7Parent; end         # gp.inh ALSO fires for child
                                      # (inherited resolves via parent's
                                      # singleton ancestor chain)
