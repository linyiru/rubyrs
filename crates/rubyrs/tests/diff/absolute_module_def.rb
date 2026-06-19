# `module ::Foo` / `class ::Bar` inside a class body defines at TOP
# LEVEL, ignoring the enclosing lexical scope (Sinatra helpers_test
# defines `module ::HelperOne` inside `class HelpersTest`).
class Outer
  module ::TopMod; def hi; "from-topmod"; end; end
  class ::TopCls; def greet; "from-topcls"; end; end
end
p defined?(TopMod)
p defined?(TopCls)
p Outer.constants.include?(:TopMod)
p Outer.constants.include?(:TopCls)
p ::TopCls.new.greet
class Includer; include ::TopMod; end
p Includer.new.hi
# absolute reference from inside a nested class resolves to top level
class Other; def fetch; ::TopMod; end; end
p Other.new.fetch.instance_method(:hi).class
