# Constant resolution through prepend, superclass-includes-M, and
# the CRuby subtlety where constant lookup orders the class BEFORE
# its prepended modules (unlike METHOD lookup, where prepend wins).

# --- prepend: own const beats prepended module's const ---
module Pre
  X = "Pre::X"
  Y = "Pre::Y"      # only in Pre
end
class K
  X = "K::X"        # own AND prepended -> own wins for constants
  prepend Pre
  def x = X
  def y = Y
end
p K.new.x           # "K::X"   (own beats prepend, const semantics)
p K::X              # "K::X"
p K.new.y           # "Pre::Y" (only in prepend -> found via ancestor)
p K::Y              # "Pre::Y"

# Method lookup still honours prepend (sanity: const != method order)
module PreM
  def m = "PreM#m"
end
class L
  prepend PreM
  def m = "L#m"
end
p L.new.m           # "PreM#m" (prepend wins for METHODS)

# --- superclass includes M: subclass sees M's const by bare name ---
module Shared
  S = "Shared::S"
end
class Parent
  include Shared
end
class Child < Parent
  def s = S         # M is an ancestor of Child via Parent
end
p Child.new.s       # "Shared::S"
p Child::S          # "Shared::S"

# --- three-level ancestor ordering (prepend > include) ---
module P2
  A = "P2::A"
  B = "P2::B"
end
module I2
  B = "I2::B"       # also in P2 -> P2 (prepend) should win
  C = "I2::C"
end
class Z
  C2 = "Z::C2"
  prepend P2
  include I2
  def a = A
  def b = B
  def c = C
end
p Z.new.a           # "P2::A"
p Z.new.b           # "P2::B" (prepend beats include)
p Z.new.c           # "I2::C"
p Z::A              # "P2::A"
p Z::B              # "P2::B"
p Z::C              # "I2::C"
