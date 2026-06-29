# ADR 0034 Step 1 (piece 5) — a value-producing `stmt if cond` (statement-modifier)
# leaves the then-value and the else-`nil` merged on the stack, then discards it. The
# JIT now allows that Nil/X kind mismatch WHEN the merged value is immediately popped
# (the target block's first op is Pop) — so a recursive walk with `c += f(x) if cond`
# compiles. A USED merge value (`(x if c) + 1`) still requires an exact match. Parity
# must hold interpreter == JIT == CRuby.

# The exact rubocop AST-walk skeleton shape (`c += walk(x) if x.class == Node`).
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
    c += walk(x) if x.class == Node      # statement-modifier: Int (then) / nil (else)
    i += 1
  end
  c
end
leaf = Node.new([])
root = Node.new([leaf, Node.new([leaf, leaf, :sym]), :other])
p walk(root)
p walk(leaf)

# A plainer `stmt if cond` accumulator in a loop.
def count_pos(arr)
  c = 0
  i = 0
  while i < arr.length
    c += 1 if arr[i] > 0
    i += 1
  end
  c
end
@nums = [3, -1, 4, -1, 5, 0, 9]
def run; count_pos(@nums); end   # @nums via ivar -> arr param... use a top-level helper
p [3, -1, 4, -1, 5, 0, 9].each_with_object([0]) { |x, a| a[0] += 1 if x > 0 }[0]
