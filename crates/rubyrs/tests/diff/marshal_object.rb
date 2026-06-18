# Generic object (`o`-tag) + exception (`:mesg`/`:bt`) marshalling:
# byte-compatible dump, deep copy, exception state round-trip.
class Box; def initialize(v); @v = v; end; def v; @v; end; end
b = Box.new(42)
p Marshal.dump(b).bytes
r = Marshal.load(Marshal.dump(b))
p [r.v, r.class.name]
# multi-ivar object round-trips (byte ORDER may differ from CRuby —
# rubyrs ivars are name-sorted — so compare values, not dump bytes)
class Point; def initialize(x, y); @x = x; @y = y; end; def to_a; [@x, @y]; end; end
p Marshal.load(Marshal.dump(Point.new(1, 2))).to_a
# deep copy independence
orig = Box.new([1, 2])
copy = Marshal.load(Marshal.dump(orig))
copy.v << 9
p orig.v
p orig.equal?(copy)
# shared object reconstructs shared identity
sh = Box.new(7)
rr = Marshal.load(Marshal.dump([sh, sh]))
p rr[0].equal?(rr[1])
# exception with explicit message (byte-compatible)
e = RuntimeError.new("boom")
p Marshal.dump(e).bytes
r2 = Marshal.load(Marshal.dump(e))
p [r2.message, r2.class.name, r2.is_a?(RuntimeError)]
# no-arg exception → :mesg nil (byte-compatible), round-trips to class name
e4 = RuntimeError.new
p Marshal.dump(e4).bytes
p Marshal.load(Marshal.dump(e4)).message
# exception carrying a user ivar
e2 = RuntimeError.new("x"); e2.instance_variable_set(:@k, 5)
r3 = Marshal.load(Marshal.dump(e2))
p [r3.message, r3.instance_variable_get(:@k)]
# user-defined exception subclass
class MyError < StandardError; end
me = Marshal.load(Marshal.dump(MyError.new("nope")))
p [me.message, me.class.name, me.is_a?(StandardError)]
# object nested in a hash
p Marshal.load(Marshal.dump({box: Box.new(99)})).fetch(:box).v
