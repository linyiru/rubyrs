# `expr::CONST` where `expr` is a RUNTIME value (not a constant path)
# resolves CONST on the value's own class/ancestry — NOT lexically.
# The classic shape is `self.class::CONST` in a base-class method: it
# must see the RUNTIME subclass's override, not the base's constant.
# (kramdown-gfm's `self.class::FENCED_CODEBLOCK_MATCH` depends on this
# to pick the GFM `[~`]` fence regex over the base `~`-only one.)

class Base
  K = "base-K"
  def via_self_class; self.class::K; end
  def via_local; k = self.class; k::K; end
end
class Sub < Base
  K = "sub-K"
end
class InheritsK < Base; end

p Base.new.via_self_class       # "base-K"
p Sub.new.via_self_class        # "sub-K"   (runtime subclass override wins)
p Sub.new.via_local             # "sub-K"
p InheritsK.new.via_self_class  # "base-K"  (inherited, no override)

# Module-nested + deeper.
module M
  class P
    T = "P-T"
    def t; self.class::T; end
  end
  class Q < P
    T = "Q-T"
  end
end
p M::P.new.t                    # "P-T"
p M::Q.new.t                    # "Q-T"

# Local/expr base directly.
k = Sub
p k::K                          # "sub-K"
arr = [Base, Sub]
p arr[1]::K                     # "sub-K"

# Missing const on the runtime base still raises NameError.
begin
  Sub.new.class::NOPE
rescue NameError
  puts "missing ok"
end
