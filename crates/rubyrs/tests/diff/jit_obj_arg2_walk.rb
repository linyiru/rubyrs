# ADR 0034 pieces 1+8 — the 2-ARG method ABI + 2-arg Object+Int self-recursion.
# A recursive Object-tree walk that threads an Int ACCUMULATOR as a second param
# (`walk(node, acc)` → `walk(child, acc)`), guarding each heterogeneous child with
# `x.class == Const`. Parity must hold interpreter == JIT == CRuby, including the
# overflow deopt (a huge seed that promotes to Bignum) and the polymorphic case.

class Node
  attr_reader :children
  def initialize(children); @children = children; end
end

def walk(node, acc)
  acc += 1
  kids = node.children
  i = 0
  n = kids.length
  while i < n
    x = kids[i]
    acc = walk(x, acc) if x.class == Node   # 2-arg Object+Int self-recursion
    i += 1
  end
  acc
end

leaf = Node.new([])
# Heterogeneous children: Nodes AND Symbols (Symbols must NOT recurse).
root = Node.new([leaf, Node.new([leaf, leaf, :sym]), :other, Node.new([])])
p walk(root, 0)               # count Node objects, threaded through acc
p walk(leaf, 0)               # 1 (no children)
p walk(Node.new([:a, :b]), 0) # 1 (all Symbol children, none recurse)
p walk(root, 1000)            # non-zero seed threads correctly

# Polymorphic node class: a subclass is NOT `== Node` (exact class), so its
# instances don't recurse — must match CRuby exactly.
class Sub < Node; end
p walk(Node.new([Sub.new([leaf, leaf]), leaf]), 0)

# A deeper tree to exercise real recursion depth.
def build(d)
  return Node.new([:leaf]) if d <= 0
  Node.new([build(d - 1), build(d - 1), :tag])
end
p walk(build(6), 0)
