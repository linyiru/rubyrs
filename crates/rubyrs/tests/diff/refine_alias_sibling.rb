# `alias_method` inside a `refine Target do … end` block where the SOURCE
# is a sibling method defined in the same refine block. CRuby's refinement
# module has Target as an ancestor, so the source resolves from the
# refinement module itself first, then falls back to Target. Surfaced by
# bridgetown-foundation-2.2.1's refine_ext/string.rb:
#   def camelize_upper = …
#   alias_method :camelize, :camelize_upper

module StrRef
  refine ::String do
    def shout = upcase + "!"
    alias_method :yell, :shout

    # `alias` keyword variant, same holder-first resolution
    def quiet = downcase
    alias hush quiet

    # source falls back to the refined class's primitive when it's not a
    # sibling (pre-existing behavior, must survive the holder-first leg)
    alias_method :up2, :upcase
  end
end

using StrRef
p "hi".shout
p "hi".yell
p "ABC".quiet
p "ABC".hush
p "abc".up2

# The other bridgetown-foundation refine-block shapes, pinned against
# regression (all were already working):

module BtShapes
  refine ::String do
    # sibling implicit-self call
    def base_x = "B(#{self})"
    def wrapper_x = base_x + "-w"

    # refined method invoked on a FRESH object from a sibling
    # (`dup.indent!(…)` shape)
    def bang_x!
      self << "!"
    end

    def banged_x = dup.bang_x!

    # sibling implicit-self call from inside a block (`.then { questionable }`)
    def inq_x = 1.then { wrapper_x }
  end
end

using BtShapes
p "x".wrapper_x
s = +"hi"
p s.banged_x
p s          # receiver unchanged; bang! hit the dup
p "y".inq_x

# define_method sibling inside refine + alias_method of it
module DmRef
  refine ::String do
    define_method(:dm_x) { "dm(#{self})" }
    alias_method :dm_y, :dm_x
  end
end
using DmRef
p "q".dm_x
p "q".dm_y

# alias_method with a MISSING source inside refine still raises NameError
# at refine-block evaluation time (message text differs between
# implementations; the class is the pinned contract).
begin
  module BadRef
    refine ::String do
      alias_method :nope2, :nope
    end
  end
  p :unreachable
rescue NameError => e
  p e.class
end

# `refine` returns a module (CRuby: a Refinement, which is a Module;
# rubyrs Tier-1: a plain anonymous module — is_a?(Module) is the
# portable contract).
module RetRef
  R = refine ::String do
    def ret_probe = :ok
  end
end
p RetRef::R.is_a?(Module)
using RetRef
p "z".ret_probe
