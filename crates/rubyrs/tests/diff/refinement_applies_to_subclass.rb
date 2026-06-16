# A refinement on a class applies to that class AND its subclasses
# (CRuby semantics). Surfaced by bridgetown-foundation's `refine ::Hash`
# `deep_dup`, called on a `HashWithDotAccess::Hash` (Hash subclass).
module Defs
  refine Hash do
    def tagged = "tagged:#{self.class}"
  end
end
module Refs; include Defs; end
using Refs
class MyHash < Hash; end
puts({}.tagged)
puts(MyHash.new.tagged)
