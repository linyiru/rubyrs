# Object#send / __send__ — dynamic dispatch by Symbol or String
# (CRuby transparently `to_sym`s a String arg).
# Differential fixture vs CRuby; landed as the highest-leverage
# subset gap surfaced by PR #95 (the extractor v0.4 dogfood —
# upstream shared specs call `obj.send(@method)` and the
# extractor emits it verbatim).

# Primitives — Symbol resolves through the usual primitive_call
# / sym_primitive path the same as a direct method call.
puts "one".send(:length)
puts "one".__send__(:length)
puts [1, 2, 3].send(:size)
puts "hello world".send(:include?, "world")
puts "hello world".send(:include?, "xyz")
puts 5.send(:+, 3)
puts [3, 1, 2].send(:sort).inspect

# User-defined classes — defined methods on an Instance, plus
# nested `send(:send, ...)` which exercises the recogniser
# re-entering itself.
class Greeter
  def greet(name)
    "hi #{name}"
  end

  def shout
    "HEY"
  end
end
g = Greeter.new
puts g.send(:greet, "world")
puts g.send(:shout)
puts g.__send__(:greet, "alias")
puts g.send(:send, :shout)

# Block form — receiver method takes a block; send must forward
# it transparently. `map { ... }` is the obvious case.
puts [1, 2, 3].send(:map) { |x| x * 10 }.inspect
puts [1, 2, 3, 4].send(:select) { |x| x.even? }.inspect

# String arg is accepted — CRuby silently `to_sym`s it.
puts "hello".send("length")
puts [10, 20].send("first")

# TypeError on neither-symbol-nor-string. CRuby's message
# inspects the offending value verbatim.
begin
  "x".send(123)
rescue TypeError => e
  puts "type ok: #{e.message}"
end
begin
  "x".send(nil)
rescue TypeError => e
  puts "type ok: #{e.message}"
end

# ArgumentError on zero args — same shape CRuby raises.
begin
  "x".send
rescue ArgumentError => e
  puts "argc ok"
end

# NoMethodError when the resolved name doesn't exist — must
# go through the normal lookup path's miss handler (i.e. NOT
# pretend `send` itself is missing).
begin
  "x".send(:no_such_method_xyz)
rescue NoMethodError => e
  puts "missing ok"
end

# Visibility bypass — CRuby allows `send` / `__send__` to invoke
# private and protected methods even with an explicit receiver.
class Vault
  def reveal
    secret
  end

  private

  def secret
    "🤐"
  end
end
v = Vault.new
begin
  v.secret
rescue NoMethodError
  puts "direct private blocked"
end
puts v.send(:secret)
puts v.__send__(:secret)
# The bypass is single-shot — `reveal` calls `secret` internally
# and that internal call must still be a normal private dispatch
# (allowed because the call is implicit-self), but if we leaked
# the flag a subsequent direct call to `v.secret` would succeed.
puts v.reveal
begin
  v.secret
rescue NoMethodError
  puts "still blocked"
end

# User-defined `def send` — CRuby's reserved-name rule: `send`
# is overridable, `__send__` is not.
class HasOwnSend
  def send(*args)
    "intercepted #{args.inspect}"
  end

  def hidden
    "real_hidden"
  end
end
h = HasOwnSend.new
puts h.send(:hidden)           # → user's send wins
puts h.__send__(:hidden)        # → built-in re-aim wins

# TypeError inspect — non-primitive target's message uses
# Value::to_inspect so an array renders as `[1, 2]` rather
# than the bare type name.
begin
  "x".send([1, 2])
rescue TypeError => e
  puts "type ok: #{e.message}"
end

# Block-form bypass must also be single-shot — `send(:map) { ... }`
# (a block-form re-aim that goes through `do_call_block`) must not
# leak the visibility flag into a subsequent direct call.
[1, 2].send(:map) { |n| n }
begin
  v.secret
rescue NoMethodError
  puts "still blocked after block-form send"
end
