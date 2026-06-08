# Bare `Str::Double` where `Str` is a const ALIAS reachable only through
# an included module — rouge's lexers do exactly this: a Lexer
# `include Token::Tokens`, then references `Str::Double` where
# `Tokens::Str` aliases `Tokens::Literal::String` (which has a nested
# `Double`). The flat key `<ancestor>::Str::Double` doesn't exist, so
# resolution must take the first segment (`Str`) through the ancestor
# walk to its class, then resolve the rest (`Double`) on that class.

module Toks
  module Inner
    class A
      class Leaf; end
      X = 7
    end
    # alias const: Ali points at the class A
    Ali = Inner::A
  end
end

class Base
  include Toks::Inner
end

class Lexer < Base
  # bare `Ali::Leaf` — Ali via include, Leaf nested under the aliased class
  p Ali::Leaf.name                 # "Toks::Inner::A::Leaf"
  p Ali::X                         # 7
  M = { :leaf => Ali::Leaf }       # the cmake.rb STATES_MAP shape
  p M[:leaf].name                  # "Toks::Inner::A::Leaf"
end

# Still works at the qualified call site (C::Ali::Leaf shape).
p Base::Ali::Leaf.name             # "Toks::Inner::A::Leaf"
# A genuinely-missing nested const still raises.
begin
  Lexer.const_get(:nope) rescue (Lexer.class_eval { Ali::Nope })
rescue NameError => e
  puts "missing ok"
end
