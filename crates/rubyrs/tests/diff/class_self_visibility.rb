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
## Runtime semantics (updated by S3): bodies containing a bare
## visibility modifier now route to the REAL eigenclass-body
## path (`Op::OpenSingletonClass`, self = the metaclass) — the
## modifier flips the visibility entry the op pushed, and
## `Op::DefMethod`'s shell-redirected install stamps subsequent
## defs with it.
##
## The KNOWN GAP this header used to carry — singleton-method
## dispatch not enforcing Private/Protected — is CLOSED (S3
## item e): `Foo.secret` now raises the CRuby NoMethodError for
## private AND protected class methods, with the metaclass-kin
## exemption for subclass callers. Enforcement is pinned by
## eigenclass_protected.rb; this fixture keeps pinning the
## admission + leak/unwind isolation invariants below.

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

## The PUBLIC methods around the `private` span stay callable
## (the private span's enforcement is pinned in
## eigenclass_protected.rb). Both rubyrs and CRuby agree on the
## values returned by direct call.
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

## Exception-unwind leak guard (PR #233 code-review round 2 #4):
## a raise inside a `class << self` body that's rescued OUTSIDE
## must still pop the visibility scope, OR subsequent defs in
## the outer class would inherit the singleton body's last
## visibility setting. The translator wraps the body in
## `Begin { ensure: [PopClassVisibility] }` so the pop runs on
## both normal and exceptional exit.
##
## Simulated via `class_eval` (rubyrs handles class_eval's
## block-as-class-body path). A raise inside the eval'd
## `class << self` body propagates up; the outer rescue
## catches it. Then we define a fresh class and check that
## its instance method is public.
exception_handled = begin
  Object.class_eval do
    class << self
      private
      ## Method call that raises NoMethodError (no such method
      ## exists on the singleton class). The if-modifier
      ## wrapper from PR #218 admits the syntax; the runtime
      ## dispatch raises. Both rubyrs and CRuby raise here.
      no_such_helper_method_for_unwind_test if true
    end
  end
  "did-not-raise"
rescue NoMethodError, NameError
  "rescued"
end
puts "exception-handled=#{exception_handled}"

## After rescue, define a fresh class — instance method must
## be Public. If the singleton body's `private` leaked an
## extra entry into class_visibility_stack and stayed there
## after the exception, this method would be installed as
## Private. The ensure-Pop in the AST translator's
## SingletonClassNode handler is what prevents that.
class PostUnwindLeakGuard
  def visible_after_unwind
    "post-unwind"
  end
end
puts "post-unwind=#{PostUnwindLeakGuard.new.visible_after_unwind}"
puts "respond-post-unwind=#{PostUnwindLeakGuard.new.respond_to?(:visible_after_unwind)}"
