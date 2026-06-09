# Refinements: `refine(Target) do … end` inside a module + `using M` to
# activate. (Tier-1: activation is global from the `using` point, not
# lexically scoped per file/module — equivalent for a single-file script;
# see SUBSET.md.)

module StrExt
  refine String do
    def shout; upcase + "!"; end
    def whisper; downcase; end
  end
  refine Integer do
    def double; self * 2; end
  end
end

using StrExt
p "Hi".shout
p "LOUD".whisper
p 21.double

# refinement methods take args, call other methods on self
module ArrExt
  refine Array do
    def second; self[1]; end
    def take_pairs(n); first(n * 2).each_slice(2).to_a; end
  end
end
using ArrExt
p [10, 20, 30].second
p [1, 2, 3, 4, 5].take_pairs(2)

# refinement OVERRIDES a native method (within the using scope)
module Override
  refine String do
    def length; 999; end
  end
end
using Override
p "abc".length          # 999 (refined wins over the native primitive)
p "abc".upcase          # native, unrefined

# refining a USER class
class Widget
  def initialize(n); @n = n; end
  def base; @n; end
end
module WExt
  refine Widget do
    def boosted; base * 100; end
  end
end
using WExt
p Widget.new(3).boosted

# a refinement calls another active refinement on self
module IntExt2
  refine Integer do
    def triple; self + double; end   # double from StrExt, also active
  end
end
using IntExt2
p 5.triple              # 5 + 10

# `using` an empty module is a harmless no-op
module Empty; end
using Empty
p "ok".upcase

# a refinement that's never `using`'d is not active
module Unused
  refine String do
    def never_called; "nope"; end
  end
end
p "x".respond_to?(:never_called)   # false

# native methods entirely unaffected
p [1, 2, 3].map { |x| x * 2 }
p({ a: 1 }.keys)
