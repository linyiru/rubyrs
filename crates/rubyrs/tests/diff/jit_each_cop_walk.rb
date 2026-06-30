# ADR 0034 gap A — the FULL cop-like body walked via `.each` (not `while`): a 2-arg
# `walk(node, counts)` that does Hash counting (Symbol keys), `is_a?`, a Bool predicate
# (`send_type?`), AND `&&` (`name.is_a?(Symbol) && name.length > 8`), recursing inside
# `node.children.each { |c| walk(c, counts) if c.is_a?(Node) }`. This is how real RuboCop
# cops walk. The `&&` Dups a Bool across a block edge — exercises the per-kind block-param
# type fix. Parity must hold interpreter == JIT == CRuby.

class Node
  attr_reader :type, :children
  def initialize(type, children); @type = type; @children = children; end
  def send_type?; @type == :send; end
  def if_type?;   @type == :if;   end
end

def walk(node, counts)
  t = node.type
  counts[t] = (counts[t] || 0) + 1
  if node.send_type?
    name = node.children[1]
    counts[:long_method] = (counts[:long_method] || 0) + 1 if name.is_a?(Symbol) && name.length > 8
  elsif node.if_type?
    counts[:cond] = (counts[:cond] || 0) + 1 if node.children.length >= 3
  end
  node.children.each { |c| walk(c, counts) if c.is_a?(Node) }
end

leaf = Node.new(:lit, [1])
# A :send node: children[1] is the method-name Symbol (drives the `&&` / long_method path).
short = Node.new(:send, [leaf, :short, leaf, leaf])
long  = Node.new(:send, [leaf, :method_name_long, leaf])  # name.length > 8
iff   = Node.new(:if,   [leaf, leaf, leaf])                # children.length >= 3 -> :cond
root  = Node.new(:begin, [short, long, iff, :a_symbol, leaf])

c = {}; walk(root, c); p c
c = {}; walk(leaf, c); p c
c = {}; walk(long, c); p c

# Deeper tree, real recursion depth through the inlined each loop.
def build(d)
  return Node.new(:lit, [d]) if d <= 0
  Node.new(:send, [build(d - 1), :"deep_method_#{d}", build(d - 1)])
end
c = {}; walk(build(7), c); p c
