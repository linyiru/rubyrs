# ADR 0035 Phase 5 — inline self-ivar reads (Int `@v` + Object-ivar receiver `@l.sum`) via
# the in-method scan, no per-ivar primitive. Exercises: the treesum shape, a class whose
# ivars are set in a DIFFERENT order (the scan must find by sym, not assume a slot), and an
# object missing an ivar (scan miss → deopt). Parity interpreter == JIT == CRuby.

class Tree
  def initialize(v, l, r); @v = v; @l = l; @r = r; end
  def sum(d)
    return @v if d == 0
    @v + @l.sum(d - 1) + @r.sum(d - 1)
  end
end
def build(v, d)
  return Tree.new(v, nil, nil) if d == 0
  Tree.new(v, build(v * 2 + 1, d - 1), build(v * 2 + 2, d - 1))
end
root = build(1, 8)
p root.sum(8)
p root.sum(8)  # warm + fast path

# A class that sets the SAME ivars in a DIFFERENT order — the scan must match by name.
class Rev
  def initialize(v, l, r); @r = r; @l = l; @v = v; end   # reverse insertion order
  def sum(d)
    return @v if d == 0
    @v + @l.sum(d - 1) + @r.sum(d - 1)
  end
end
def rbuild(v, d)
  return Rev.new(v, nil, nil) if d == 0
  Rev.new(v, rbuild(v * 2 + 1, d - 1), rbuild(v * 2 + 2, d - 1))
end
rr = rbuild(1, 6)
p rr.sum(6)

# A reader whose ivar is sometimes UNSET (scan miss → nil → deopt path).
class Maybe
  def initialize(set); @x = 10 if set; end
  def read; @x; end
end
p Maybe.new(true).read    # 10
p Maybe.new(false).read   # nil
10.times { p Maybe.new(true).read }
