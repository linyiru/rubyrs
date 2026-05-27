## `class << self` body now accepts bare `private` / `public` /
## `protected` visibility modifiers. Closes TRY_RUNS pass-9.7
## layer #14.
##
## sinatra/base.rb:1690 has `class << self; ...; private; ...;
## end` where subsequent helpers are hidden from external
## callers. Before this fix the bare visibility modifier
## tripped the body translator's catch-all NotImplementedError
## at class-body load time, blocking everything past line 1690.
##
## Scope (this PR): translator admission only. The runtime
## semantics:
##   1. `private` at body top level translates as a regular
##      method call (Expr::Call with no receiver).
##   2. At runtime, do_call's visibility-from-name arm fires
##      because self_val is the surrounding class (Value::Class).
##   3. The arm mutates `class_visibility_stack.last_mut()` to
##      the new visibility.
##   4. Subsequent `def` (DefSingletonMethod op) reads
##      `class_visibility_stack.last()` and stamps the method
##      with that visibility.
## Verified via debug trace: methods following `private` ARE
## installed with Visibility::Private.
##
## KNOWN GAP (separate PR): singleton-method dispatch
## (do_call's `lookup_class_singleton_method` arm) does NOT
## currently enforce the Private/Protected visibility check
## that instance dispatch enforces — so `Foo.secret` succeeds
## even when `:secret` is marked Private. The translator-level
## acceptance (this PR) doesn't introduce that gap; it's
## pre-existing in the singleton dispatch path. Fixing it
## requires adding visibility enforcement at line ~2913 of
## dispatch.rs, alongside CRuby's "self_recv vs explicit"
## semantic. Flagged for follow-up.

class WithVisModifier
  class << self
    def public_one
      "pub-one"
    end

    private

    def hidden_helper
      "hidden"
    end

    public

    def visible_again
      "pub-two"
    end
  end
end

## Class-body load completed — sinatra would have died at the
## bare `private` before this fix.
puts "loaded=true"

## The methods ARE installed (and callable; the Private mark
## doesn't currently enforce on singleton dispatch — separate
## gap noted above). Both rubyrs and CRuby agree on the values
## returned by direct call.
puts "public_one=#{WithVisModifier.public_one}"
puts "visible_again=#{WithVisModifier.visible_again}"

## Visibility leakage guard (PR #233 code-review #2): a bare
## `private` inside `class << self` body MUST NOT affect the
## visibility of subsequent INSTANCE methods defined after the
## `class << self ... end`. CRuby treats `class << self` as
## its own body with its own initial-Public visibility scope;
## rubyrs needed `PushClassVisibilityPublic` / `PopClassVisibility`
## opcodes to replicate that isolation. The check: define a
## class with `private` inside `class << self`, then add a
## public instance method after the singleton body. The
## instance method must be callable from outside.
class LeakGuard
  class << self
    private
    def hidden_singleton; end
  end
  def public_instance_method
    "still-public"
  end
end

puts "post-singleton-instance=#{LeakGuard.new.public_instance_method}"
puts "respond-to-instance=#{LeakGuard.new.respond_to?(:public_instance_method)}"
