# `Mod.module_eval(string)` / `class_eval(string)` runs with the
# receiver as its lexical cref, so bare constants in the eval'd code
# resolve through the receiver's namespace (and its outer scopes).
# Surfaced by rss's dublincore.rb module_eval'd accessors referencing
# bare `Element` (→ RSS::Element).
module Lib
  class Element; def self.tag; "el"; end; end
  WIDTH = 80
  module Inner
    KIND = :inner
  end
  Inner.module_eval(<<-RUBY)
    def self.via_eval; Element.tag; end       # outer-scope const
    def self.width; WIDTH; end                # outer-scope const
    def self.own; KIND; end                   # receiver's own const
  RUBY
end
p Lib::Inner.via_eval     # "el"
p Lib::Inner.width        # 80
p Lib::Inner.own          # :inner

# class_eval string form on a class
class Box; SIDE = 3; end
Box.class_eval("def self.area; SIDE * SIDE; end")
p Box.area                # 9

# bare eval (no class_ctx) stays toplevel — a bare const there resolves
# at the top level, not inside any class
TOP = :top
p eval("TOP")             # :top
