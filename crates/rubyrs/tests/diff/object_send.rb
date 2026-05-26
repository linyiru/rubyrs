# Object#send / __send__ — dynamic dispatch by Symbol name.
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
