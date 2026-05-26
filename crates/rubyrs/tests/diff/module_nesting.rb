# `Module.nesting` — reflection returning the lexical scope chain
# at the call site, innermost-first. CRuby's canonical use is for
# explaining cref-walk behaviour; rubyrs uses the same per-proto
# lexical_scope SymId list that drives `Op::LoadConstChain` to
# answer the reflection call.

# Top level: empty.
puts Module.nesting.inspect

# Single module.
module Foo
  puts Module.nesting.inspect
end

# Two-level: module containing a nested module.
module Outer
  module Inner
    puts Module.nesting.inspect
  end
end

# Three-level: module > module > class.
module A
  module B
    class C
      puts Module.nesting.inspect

      # Method body sees the same scope it was compiled in.
      def reflect
        Module.nesting
      end
    end
  end
end
puts A::B::C.new.reflect.inspect

# Block bodies inherit the surrounding scope (same lexical_scope
# threading the compiler already does for class_path).
module Wrap
  class K
    def run_block
      [1].each do
        return Module.nesting
      end
    end
  end
end
puts Wrap::K.new.run_block.inspect

# Sibling classes share the parent module but not each other.
module Sib
  class X
    def here; Module.nesting; end
  end
  class Y
    def here; Module.nesting; end
  end
end
puts Sib::X.new.here.inspect
puts Sib::Y.new.here.inspect
