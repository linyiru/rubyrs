# Module#prepend — natural follow-up to PR #102 (Class-level
# @ivars). `tilt.rb:27` was the next blocker on the dogfood path.

# Basic dispatch order: M.greet wins over C.greet, super defers
# to C's original.
module M
  def greet
    "M: " + super
  end
end

class C
  def greet
    "C"
  end
  prepend M
end

puts C.new.greet                 # "M: C"

# `ancestors` first/last positioning — full inspect would include
# Object/Kernel/BasicObject which rubyrs doesn't model. Names of
# the user-visible front of the chain are still ground-truth for
# dispatch order.
puts C.ancestors[0].name         # "M"
puts C.ancestors[1].name         # "C"

# is_a? walks prepends too, so `include?` answers true for both
# included and prepended modules.
puts C.new.is_a?(M)
puts C.include?(M)

# Prepend stacked with include — CRuby's lookup order is
#   prepends → own methods → most-recent include → ... → super.
module P; def foo; "P+" + super; end; end
module IA; def foo; "A"; end; end
module IB; def foo; "B+" + super; end; end

class D
  include IA       # IA listed first, so IB is the "more recent" include
  include IB
  prepend P
end
# Chain at D: P → D-own (no foo) → IB → IA. So:
puts D.new.foo                   # "P+B+A"
puts D.ancestors[0].name         # "P"
puts D.ancestors[1].name         # "D"
puts D.ancestors[2].name         # "IB"
puts D.ancestors[3].name         # "IA"

# Explicit-receiver form on an already-defined class — should
# work the same as inside-body. CRuby allows both.
module Q
  def hi
    "Q-" + super
  end
end
class E
  def hi
    "E"
  end
end
E.prepend(Q)
puts E.new.hi                    # "Q-E"

# Idempotent — prepending the same module twice doesn't duplicate
# it in the chain.
module S
  def s; "s"; end
end
class F
  prepend S
  prepend S
end
# If S were duplicated by the second `prepend S`, ancestors[1]
# would be S again. Both interpreters render F at [1] because
# the chain stays [S, F, ...].
puts F.ancestors[0].name         # "S"
puts F.ancestors[1].name         # "F"

# `super` from an included module's method also walks correctly
# (was broken pre-PR alongside the prepend case — same ancestor-
# chain fix).
module IncMod
  def greet; "from-inc"; end
end
class G
  include IncMod
  def greet
    "G: " + super
  end
end
puts G.new.greet                 # "G: from-inc"
