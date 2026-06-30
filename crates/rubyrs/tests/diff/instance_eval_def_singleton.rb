# `instance_eval` / `instance_exec` with a Class/Module receiver: a bare
# `def name; end` inside defines a SINGLETON (class) method on the receiver
# (CRuby), not a toplevel method. regexp_parser wires terminal? via
# `Subexpression.instance_eval { def terminal?; false; end }`, which
# RuboCop's Style/RedundantRegexpEscape relies on (tree traversal gates on
# terminal?).

class Foo; end
Foo.instance_eval { def bar; 42; end }
puts Foo.respond_to?(:bar)              # true
puts Foo.bar                            # 42
puts Foo.singleton_methods.include?(:bar)  # true

# the inheritance shape from regexp_parser
module ClassMethods; def terminal?; true; end; end
class Base; extend ClassMethods; end
class Sub < Base; end
Sub.instance_eval { def terminal?; false; end }
puts Sub.terminal?                      # false (singleton overrides extended default)
puts Base.terminal?                     # true  (unaffected)

# instance_exec form
class Baz; end
Baz.instance_exec { def qux; "q"; end }
puts Baz.qux                            # q

# Module receiver
module Mod; end
Mod.instance_eval { def helper; :h; end }
puts Mod.helper                         # h

# def with args + body still works as a class method
class Calc; end
Calc.instance_eval do
  def add(a, b) = a + b
end
puts Calc.add(2, 3)                     # 5

# a NORMAL (non-instance_eval) block def is unaffected — still toplevel
[1].each { def toplevel_m; :t; end }
puts toplevel_m                         # t  (top-level method, as before)

# instance_eval that only READS state (the common DSL case) is unchanged
config = Object.new
config.instance_variable_set(:@x, 7)
val = config.instance_eval { @x }
puts val                                # 7
