# Proc#binding — a Binding over the block's scope (self + closed-over
# locals). erubi's test harness does eval(engine.src, block.binding).
# NB: rubyrs's Binding SNAPSHOTS locals at creation (same as
# Kernel#binding), so post-creation mutation through a sibling proc is
# not reflected — a documented subset gap; erubi evals once against
# already-assigned locals, so it's unaffected.
p proc{}.respond_to?(:binding)
p proc{}.binding.class

def grab(&b); b.binding; end
x = 10
y = 20
bnd = grab {}
p eval("x + y", bnd)

# self carries through
class Ctx
  def initialize; @v = 99; end
  def make; proc {}.binding; end
  def val; @v; end
end
c = Ctx.new
p eval("val", c.make)
p eval("@v", c.make)

# locals captured by the block (assigned before binding)
def outer
  msg = "hi"
  n = 3
  -> {}.binding
end
b2 = outer
p eval("msg * n", b2)
