# ADR 0034 gap A — the `.each`-form recursive Object-tree walk goes native: a method
# whose body is `node.children.each { |x| c += walk(x) if x.class == Node }` is rewritten
# (the each-block inlined as a while loop, the block sharing the method frame) and
# compiles like the explicit-`while` form. Real RuboCop cops walk via `.each` /
# `each_child_node`, not `while`. Parity must hold interpreter == JIT == CRuby.

class Node
  attr_reader :children
  def initialize(children); @children = children; end
end

def walk(node)
  c = 1
  node.children.each do |x|
    c += walk(x) if x.class == Node
  end
  c
end

leaf = Node.new([])
# Heterogeneous children: Nodes AND Symbols (Symbols must NOT recurse).
root = Node.new([leaf, Node.new([leaf, leaf, :sym]), :other, Node.new([])])
p walk(root)                  # count Node objects via .each recursion
p walk(leaf)                  # 1 (no children — the each loop body never runs)
p walk(Node.new([:a, :b]))    # 1 (all Symbol children, none recurse)

# Polymorphic node class: a subclass is NOT `== Node` (exact class), so its instances
# don't recurse — must match CRuby exactly.
class Sub < Node; end
p walk(Node.new([Sub.new([leaf, leaf]), leaf]))

# A deeper tree to exercise real recursion depth through the inlined each loop.
def build(d)
  return Node.new([:leaf]) if d <= 0
  Node.new([build(d - 1), build(d - 1), :tag])
end
p walk(build(6))

# An EMPTY-children Node (the each loop runs zero times).
p walk(Node.new([]))
