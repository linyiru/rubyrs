# ADR 0034 Step 1 (pieces 3+4) — a recursive Object-tree walk goes native: an
# Object-param method that indexes an obj-call Array of HETEROGENEOUS elements
# (`x = kids[i]`), guards each with `x.class == Const`, and RECURSES on the matching
# ones (`walk(x)`, an Object-arg self-call). The rubocop AST-walk shape. Parity must
# hold interpreter == JIT == CRuby, including the deopts.

class Node
  attr_reader :children
  def initialize(children); @children = children; end
end

def walk(node)
  c = 1
  kids = node.children
  i = 0
  n = kids.length
  while i < n
    x = kids[i]
    c = c + (x.class == Node ? walk(x) : 0)   # ternary: Int/Int merge
    i += 1
  end
  c
end

leaf = Node.new([])
# Heterogeneous children: Nodes AND Symbols (Symbols must NOT recurse).
root = Node.new([leaf, Node.new([leaf, leaf, :sym]), :other, Node.new([])])
p walk(root)                  # count Node objects in the tree
p walk(leaf)                  # 1 (no children)
p walk(Node.new([:a, :b]))    # 1 (all Symbol children, none recurse)

# Polymorphic node class: a subclass is NOT `== Node` (exact class), so its instances
# don't recurse — must match CRuby exactly.
class Sub < Node; end
p walk(Node.new([Sub.new([leaf, leaf]), leaf]))   # Sub child not == Node -> not recursed

# A deeper tree to exercise real recursion depth.
def build(d)
  return Node.new([:leaf]) if d <= 0
  Node.new([build(d - 1), build(d - 1), :tag])
end
p walk(build(6))
