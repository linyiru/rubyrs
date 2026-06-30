# A fully-qualified constant path (`A::B::C`) referenced from inside a
# nested lexical scope whose first segment matches an enclosing module —
# the final const living in its owner's per-class consts, not as a global
# joined key. Driver: rubocop-ast's RuboCop::AST::Node#updated reads
# RuboCop::AST::Builder::NODE_MAP from inside RuboCop::AST::Node.
module RuboCop
  module AST
    module Builder
      NODE_MAP = { send: "S", int: "I" }
    end
    class Node
      def lookup(t); RuboCop::AST::Builder::NODE_MAP[t]; end
      def via_ast(t); AST::Builder::NODE_MAP[t]; end
      def via_builder(t); Builder::NODE_MAP[t]; end
    end
  end
end
n = RuboCop::AST::Node.new
p n.lookup(:send)
p n.via_ast(:int)
p n.via_builder(:send)
p RuboCop::AST::Builder::NODE_MAP.size
