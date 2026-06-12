# define_method bodies resolve super() and __method__ under their
# RUNTIME-installed name (the block proto's compile-time context is
# its lexical surroundings — useless for both). minitest/spec's
# before/after hooks are the motivating consumer.
class P2
  def setup; "P2-setup"; end
end
class C2 < P2
  define_method :setup do
    "C2+" + super()
  end
end
p C2.new.setup

class Q3
  define_method(:zz) { __method__ }
end
p Q3.new.zz

class P3
  def hook; "P3"; end
end
class C3 < P3
  define_method :hook do
    [1].map { super() }.first + "+C3"
  end
end
p C3.new.hook

# the spec-hook composition shape
class Base4
  def setup; @order = ["base"]; end
  attr_reader :order
end
class C4 < Base4
  define_method :setup do
    super()
    @order << "child"
  end
end
c = C4.new
c.setup
p c.order

# def-compiled methods unaffected: super still resolves the def name
class P5
  def m1; "P5#m1"; end
end
class C5 < P5
  def m1; "C5+" + super; end
end
p C5.new.m1
