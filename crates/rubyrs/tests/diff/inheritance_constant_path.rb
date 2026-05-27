## `class Foo < Bar::Baz` — superclass resolved via a constant
## path expression, not a bare constant. Surfaced as TRY_RUNS
## pass 7 layer #6 (the `alias secure? ssl?` failure inside
## `class Sinatra::Request < Rack::Request`), but the root
## cause was broader: the AST translator only accepted
## `ConstantReadNode` as a superclass — `ConstantPathNode`
## (`Foo::Bar`) silently returned `None`, so DefClass popped
## Nil and the child class lost its inheritance link entirely.
## Observable as "undefined method '...' for Object" on any
## subclass instance whose parent was named via `::`.

## Sanity: bare-name superclass keeps working.
class TP; def greet; "tp-greet"; end; end
class TC < TP; end
puts "bare: #{TC.new.greet}"

## The actual bug: top-level child, module-nested parent.
module M
  class MP; def greet; "mp-greet"; end; end
end
class CTop < M::MP; end
puts "top<-mod: #{CTop.new.greet}"

## Nested-via-path child AND module-nested parent — the
## exact shape sinatra/base.rb uses for `Sinatra::Request <
## Rack::Request`.
module N
  class NC < M::MP; end
end
puts "mod<-mod: #{N::NC.new.greet}"

## Deeper paths: `class C < A::B::C`.
module A
  module B
    class BC; def deep; "deep-hit"; end; end
  end
end
class CDeep < A::B::BC; end
puts "deep: #{CDeep.new.deep}"

## The original pass-7 layer #6 shape: alias to a method
## defined on a module-nested superclass. Previously failed
## with "undefined method `ssl?' for class `Sinatra::Request'".
module Rack
  class Request
    def ssl?; false; end
  end
end
module Sinatra
  class Request < Rack::Request
    alias secure? ssl?
  end
end
puts "alias-secure: #{Sinatra::Request.new.secure?}"

## Multiple-step inheritance via path: child's child still
## sees the ancestor via path.
module P
  class Base; def stamp; "base-stamp"; end; end
end
class Mid < P::Base; end
class Leaf < Mid; end
puts "grandchild: #{Leaf.new.stamp}"
