# `super` from an override into Object's dispatch-level builtins
# (send/__send__/public_send/===) — minitest Mock's blank-slate
# keeps passthrough overrides that super up.
class BlankIsh
  def method_missing(sym, *args)
    [:mm, sym, args]
  end
  define_method(:send) do |*args, &b|
    super(*args, &b)
  end
  define_method(:===) do |*args|
    super(*args)
  end
end
b = BlankIsh.new
p b.send(:foo, 1, 2)
p (b === b)
p (b === Object.new)
# block forwarded through the send passthrough
class BlankBlock < BlankIsh
  def real_with_block
    yield 7
  end
end
bb = BlankBlock.new
p bb.send(:real_with_block) { |x| x * 3 }
# no-block super path (def-form forwarder)
class DefSend
  def method_missing(sym, *_a); [:dmm, sym]; end
  def send(*args)
    super(*args)
  end
end
p DefSend.new.send(:zork)
# error paths of the substitution: super() with no forwarded name,
# and a non-symbol name
class BadSend
  def method_missing(*); :nope; end
  def send(*_args)
    super()
  end
  def send2(x)
    self.send(x)
  end
end
begin
  BadSend.new.send(:x)
rescue ArgumentError => e
  puts "no-name: ArgumentError"
end
class NumSend
  def method_missing(*); :n; end
  define_method(:send) { |*a, &b| super(*a, &b) }
end
begin
  NumSend.new.send(42)
rescue TypeError => e
  puts "non-sym: TypeError"
end
