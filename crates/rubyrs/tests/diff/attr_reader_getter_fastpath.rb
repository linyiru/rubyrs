# attr_reader getter fast path (ADR 0031 follow-up). `obj.foo` where
# foo is a trivial attr_reader (body `[LoadIvar(@foo), Return]`) is
# served by reading the receiver's ivar directly, skipping the frame
# push. Must resolve IDENTICALLY to the slow path: inheritance,
# missing-ivar-as-nil, explicit vs implicit self, wrong-arity error,
# a reopened/overridden getter (no longer a trivial getter -> must NOT
# take the shortcut), runtime-installed attr_reader, singleton override,
# and a frozen receiver.
class Base
  attr_reader :a, :b
  def initialize(a, b); @a = a; @b = b; end
end
class Sub < Base
  attr_reader :c
  def initialize(a, b, c); super(a, b); @c = c; end
end
o = Sub.new(1, "two", :three)
p o.a            # inherited getter
p o.b
p o.c

class Empty; attr_reader :x; end
p Empty.new.x    # uninitialized ivar reads as nil

class Pt
  attr_reader :x
  def initialize; @x = 9; end
  def via_self; self.x; end       # explicit-recv self
  def via_implicit; x; end        # implicit-self (no_recv)
end
pt = Pt.new
p pt.via_self
p pt.via_implicit
p pt.x

begin
  pt.send(:x, 1)                  # argc>0 must still raise (fast path skipped)
rescue ArgumentError
  p :argerr
end

class Pt
  def x; @x * 100; end            # reopened: body no longer a trivial getter
end
p pt.x                            # 900 — must NOT take the getter shortcut

class Dyn; def initialize; @v = 7; end; end
Dyn.send(:attr_reader, :v)        # runtime-installed attr_reader
p Dyn.new.v

s = Pt.new
def s.x; 555; end                # singleton override
p s.x

p Base.new(10, 20).freeze.a      # frozen receiver getter
