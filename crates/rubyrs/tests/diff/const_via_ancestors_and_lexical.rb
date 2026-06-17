# Constant resolution through an INCLUDED module's own table, and a
# bare `Head::Rest` reference whose head lives in an OUTER lexical
# scope. Surfaced by rexml (`class Entity < Child; include XMLTokens`,
# then `Entity::NAME` reading XMLTokens::NAME from another class).
module Lib
  module Toks
    NAME = "nm"
    KIND = :tok
  end
  class Node; end
  class Child < Node; end
  class Entity < Child
    include Toks
  end
  class Reader < Child
    # bare `Entity` resolves via lexical scope [Reader, Lib] → Lib::Entity;
    # `::NAME` then resolves through Entity's included module Toks.
    PAT = "(#{Entity::NAME})"
    def self.pat; PAT; end
    def self.kind; Entity::KIND; end
  end
end
p Lib::Reader.pat                 # "(nm)"
p Lib::Reader.kind                # :tok

# fully-qualified path: CONST in an included module of a class with a superclass
p Lib::Entity::NAME               # "nm"

# const in a plain superclass still resolves (regression guard)
class Sup; SC = 7; end
class Sub < Sup; end
p Sub::SC                         # 7

# deeper nesting: head resolved two scopes out
module Outer
  module Mid
    class Thing
      include Lib::Toks
    end
    class Other
      def self.n; Thing::NAME; end
    end
  end
end
p Outer::Mid::Other.n             # "nm"

# const directly on the class still wins over the included module
module Lib2
  module M; X = :from_module; end
  class C
    include M
    X = :from_class
  end
end
p Lib2::C::X                      # :from_class
