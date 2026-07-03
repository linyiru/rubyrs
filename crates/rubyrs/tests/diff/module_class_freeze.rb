# Module#freeze / Class#freeze are real object operations: a frozen
# module/class must report `frozen? == true` (was a no-op that returned self
# but left `frozen?` false, because only Value::Object handled the flag).
# Also: `freeze` bare inside a class body / class-method (implicit self = the
# Class), and dup drops the flag while clone keeps it.
m = Module.new
p m.frozen?            # false
p m.freeze.equal?(m)   # true (returns self)
p m.frozen?            # true

c = Class.new
c.freeze
p c.frozen?            # true

module Named; end
Named.freeze
p Named.frozen?        # true

# bare `freeze` with implicit self = the Class being defined
class BareBody
  freeze
end
p BareBody.frozen?     # true

# freeze via bare call inside a class method
class ViaMethod
  def self.lock!; freeze; end
end
ViaMethod.lock!
p ViaMethod.frozen?    # true

# dup drops frozen, clone keeps it
module Src; end
Src.freeze
p Src.dup.frozen?      # false
p Src.clone.frozen?    # true

# an unfrozen sibling is unaffected (per-class flag, not global)
module Other; end
p Other.frozen?        # false
