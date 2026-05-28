## Private / protected NoMethodError message formatting. Pre-fix
## the private-call site stuffed the full sentence ("private
## method 'X' called") into the `method` field, and the outer
## formatter wrapped it again as "undefined method `private
## method 'X' called' for Object" — a malformed double-nested
## message that broke caller code reading `e.message`. Same
## shape for the protected-call site. Also the receiver was
## rendered as "Object" instead of CRuby's "an instance of
## <ClassName>" form. (TRY_RUNS pass-10 layer #5.)
##
## Discovery context: tilt-2.7.0 `Tilt::Mapping#lookup` is
## private. Probe script called `m.lookup("foo.erb")` and got
## an uninterpretable error. CRuby's message
## ("private method 'lookup' called for an instance of
## Tilt::Mapping") is what callers regex-match on.

class Inner
  def call_a_private; private_method; end
  def call_a_protected_from_unrelated(other); other.protected_method; end

  private

  def private_method; :secret; end
end
class Inner
  def protected_method; :guarded; end
  protected :protected_method
end

## Shape 1: explicit-receiver private call. Pre-fix this raised
## NoMethodError with the malformed double-nested message;
## post-fix CRuby-shape "private method 'X' called for an
## instance of Inner".
inst = Inner.new
err = begin
  inst.private_method
  "no-raise"
rescue NoMethodError => e
  e.message
end
puts "priv-msg=#{err}"

## Shape 2: protected method called from outside the
## type-compatible chain. Same malformed-message issue
## pre-fix. CRuby: "protected method 'X' called for an
## instance of Inner".
err = begin
  inst.protected_method
  "no-raise"
rescue NoMethodError => e
  e.message
end
puts "prot-msg=#{err}"

## Shape 3: bare `private_method` from inside a method on
## the SAME instance still works (private allows implicit
## self only). Regression-prevent the recv-desc helper from
## breaking the self-recv bypass.
puts "self-priv=#{inst.call_a_private.inspect}"

## Shape 4: regression — missing-method NoMethodError keeps
## the "undefined method `X' for <type>" wording (single
## backtick + single-quote, the pre-existing CRuby-1.x style
## rubyrs has shipped for ages). Tests reading on substring
## "undefined method" must continue to match.
err = begin
  "abc".no_such_method
  "no-raise"
rescue NoMethodError => e
  e.message.start_with?("undefined method") ? "OK-undefined-shape" : "wrong-shape"
end
puts "missing-msg=#{err}"

## Shape 5: spoof-resistance. A script-controlled method name
## that happens to start with "private method " (the
## visibility-error prefix) must STILL render under the
## missing-method "undefined method" shape — visibility kind
## is a structural tag on the error variant, not a string
## prefix sniffed out of the method field. Pre-fix the
## formatter used `method.starts_with("private method ")` so
## `obj.send(:"private method 'X' called")` could misclassify
## itself as a visibility error. (code-review #291 round 2.)
err = begin
  "abc".send("private method 'X' called")
  "no-raise"
rescue NoMethodError => e
  e.message.start_with?("undefined method") ? "spoof-blocked" : "spoof-LEAKED-#{e.message}"
end
puts "spoof=#{err}"

## Shape 6: missing-method NoMethodError on an Object instance
## also renders the receiver as "an instance of <ClassName>"
## (CRuby 3.3+), matching the visibility-error shape. Pre-fix
## the visibility sites used the rich descriptor but the
## missing-method sites kept the generic "Object" type tag,
## producing inconsistent receiver wording on the SAME object.
## (code-review #291 round 2.)
## Substring check (not full message) — rubyrs uses backtick +
## apostrophe quoting (`undefined method `X'`) while CRuby 3.4
## uses both apostrophes ('X'); the receiver descriptor is
## what this shape pins.
class Bare; end
err = begin
  Bare.new.no_such_method
  "no-raise"
rescue NoMethodError => e
  e.message.include?("for an instance of Bare") ? "instance-shape" : "wrong-#{e.message}"
end
puts "missing-on-instance=#{err}"
