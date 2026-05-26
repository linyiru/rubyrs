# Cref walk for bare constant reads inside a class/module body.
# Step 2 of the key-by-qualified-name refactor (#224).
#
# After Step 1 the class table was keyed by qualified SymId,
# so `module Foo; class Plant; end; class Bar < Plant; end;
# end` would trip on `Plant` — bare LoadConst at the outer
# scope (when emitting the superclass slot for Bar) looked up
# the bare key, but Plant was stored under "Foo::Plant".
#
# Compiler now precomputes a cref chain at every const-read
# site sitting inside a non-empty class_path, ordered innermost
# scope first; runtime walks it through `classes` and
# `constants` and takes the first hit. Top-level reads stay
# on the plain `LoadConst` path.

# Sibling class reference inside a module — the classic case.
module Outer
  class Plant
    def kind; "plant"; end
  end
  class Bar < Plant
    def hello; "bar" end
  end
end
puts Outer::Bar.new.hello              # "bar"
puts Outer::Bar.new.kind               # "plant" — inherited
puts Outer::Bar.superclass.name        # "Outer::Plant" — cref-walked

# Constant read inside a method body — walks the surrounding
# class chain.
module M
  PI = 3
  class C
    TAU = 6
    def both; [PI, TAU]; end           # PI via M, TAU via M::C
    def from_outer; PI; end            # walks past C into M
  end
end
puts M::C.new.both.inspect             # "[3, 6]"
puts M::C.new.from_outer               # 3

# Top-level still resolves through the chain's outermost slot.
TOP_LEVEL = 42
module N
  class K
    def reach_top; TOP_LEVEL; end
  end
end
puts N::K.new.reach_top                # 42

# Innermost scope wins when a name is shadowed.
SHADOW = "top"
module S
  SHADOW = "middle"
  class Z
    SHADOW = "inner"
    def see; SHADOW; end
  end
end
puts S::Z.new.see                      # "inner"

# Sibling lookup INSIDE a method body — must walk to the
# enclosing module to find the sibling class.
module Sibs
  class Inner
    def value; "inner-val" end
  end
  class Outer
    def thing; Inner.new.value end     # bare `Inner` resolves to Sibs::Inner
  end
end
puts Sibs::Outer.new.thing             # "inner-val"

# Unresolved bare const still raises NameError with the bare
# name in the message.
begin
  module Empty
    class Zilch
      def boom; UnresolvedX; end
    end
  end
  Empty::Zilch.new.boom
rescue NameError => e
  puts "NE:#{e.message}"               # "uninitialized constant UnresolvedX"
end
