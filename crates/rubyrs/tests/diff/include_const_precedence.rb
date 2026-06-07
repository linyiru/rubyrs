# Precedence rules for constant resolution with included modules,
# matched byte-for-byte against CRuby's `rb_const_search`:
#
#   1. Lexical scope (Module.nesting) wins over an included module.
#   2. The includer's OWN constant wins over an included module's.
#   3. An included module (ancestor of the innermost cref) wins
#      over TOPLEVEL — toplevel/Object is searched LAST, after the
#      ancestor chain, not as part of the lexical nesting.
#   4. A name defined ONLY in the included module resolves via the
#      ancestor walk.

# --- (1) lexical OUTER scope beats included ancestor ---
module Outer
  SHARED = "Outer::SHARED (lexical)"
  module Mixin1
    SHARED = "Mixin1::SHARED (ancestor)"
  end
  class A
    include Mixin1
    def f = SHARED
  end
end
p Outer::A.new.f                 # "Outer::SHARED (lexical)"

# --- (2) own-class const beats included ---
module Mixin2
  X = "Mixin2::X"
end
class B
  include Mixin2
  X = "B::X"
  def x = X
end
p B.new.x                        # "B::X"
p B::X                           # "B::X"

# --- (3) included ancestor beats toplevel ---
BOTH = "toplevel::BOTH"
module Mixin3
  BOTH = "Mixin3::BOTH"
end
class CC
  include Mixin3
  def both = BOTH
end
p CC.new.both                    # "Mixin3::BOTH"  (ancestor, not toplevel)

# --- (4) only-in-module resolves via ancestor walk ---
TOP = "toplevel"
module Mixin4
  ONLY = "Mixin4::ONLY"
end
class DD
  include Mixin4
  def only = ONLY
end
p DD.new.only                    # "Mixin4::ONLY"
