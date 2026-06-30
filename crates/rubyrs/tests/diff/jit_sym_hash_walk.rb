# ADR 0034 pieces 2+3+4 — Kind::Symbol + Symbol-keyed Hash read/write, on the 2-arg
# method ABI. `walk(node, counts)` reads the node's `type` Symbol (jit_obj_getter_sym),
# accumulates a per-type count via `counts[t] = (counts[t] || 0) + 1` (Symbol-keyed
# hash read with `|| 0` nil-merge + write), and recurses 2-arg (Object child + Hash).
# Parity must hold interpreter == JIT == CRuby on every shape, including the deopts.

class Node
  attr_reader :type, :children
  def initialize(type, children); @type = type; @children = children; end
end

def walk(node, counts)
  t = node.type
  counts[t] = (counts[t] || 0) + 1
  kids = node.children
  i = 0
  n = kids.length
  while i < n
    c = kids[i]
    walk(c, counts) if c.class == Node
    i += 1
  end
  counts
end

leaf  = Node.new(:lit, [])
inner = Node.new(:send, [leaf, :name, Node.new(:lit, [42]), leaf])
root  = Node.new(:send, [inner, :other, leaf, Node.new(:if, [leaf, leaf])])

# Counts per type symbol — fresh hash each call (the `{}`/`|| 0` shape the JIT gates on).
p walk(root, {})
p walk(leaf, {})
p walk(Node.new(:if, [:a, :b]), {})   # Symbol children don't recurse

# Polymorphic node class: a subclass is NOT `== Node` (exact class) — its instances
# don't recurse, so they aren't counted. Must match CRuby.
class Sub < Node; end
p walk(Node.new(:send, [Sub.new(:x, [leaf, leaf]), leaf]), {})

# A deeper tree, real recursion depth + many keys.
def build(d, ty)
  return Node.new(:lit, [d]) if d <= 0
  kids = [build(d - 1, :send), build(d - 1, :if), :tag]
  Node.new(ty, kids)
end
p walk(build(7, :begin), {})

# A pre-seeded counts hash (`{ lit: 100 }`): the Symbol-keyed read finds the existing
# Int and accumulates from it — `lit` counts up from 100.
p walk(build(2, :send), { lit: 100 })

# DEOPT correctness: a DEFAULTED hash (`Hash.new(10)`) — an absent key reads 10, not
# nil, so `counts[t] || 0` yields 10 (then +1). The JIT read deopts on the defaulted
# absence and the interpreter produces the default-based count.
p walk(build(2, :send), Hash.new(10))
