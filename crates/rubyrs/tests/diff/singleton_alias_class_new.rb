# `singleton_class.send :alias_method, :[], :new` — aliasing the
# class-level builtin `new` (lives in Rust dispatch, not a Ruby-defined
# singleton method) into a `[]` constructor shorthand. Surfaced by
# concurrent-ruby's LockFreeStack::Node (`Node[nil, nil]`).
class Node
  attr_reader :value, :next_node
  def initialize(value, next_node)
    @value = value
    @next_node = next_node
  end
  singleton_class.send :alias_method, :[], :new
end

n = Node[1, nil]
puts n.value
puts n.next_node.inspect
puts n.class
puts(Node.respond_to?(:[]))

# nested form: Node[a, Node[b, EMPTY]]
m = Node[10, Node[20, nil]]
puts m.value
puts m.next_node.value
