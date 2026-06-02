# Module.included(base) / Module.prepended(base) hooks — fire
# on every `include M` / `prepend M` call. Receiver of the hook
# is the module being included/prepended; argument is the target
# class. CRuby fires the hook on every call regardless of whether
# the ancestor chain mutates — case (4) below pins that idempotent
# re-includes still re-fire the callback. ActiveSupport::Concern,
# Rails plugin systems, and Sinatra extensions all hinge on this
# hook.

# (1) included — body-form (`class Foo; include M; end`).
module MA
  def self.included(base)
    puts "MA.included(#{base.name})"
    base.instance_variable_set(:@from_hook, :ma)
  end
end
class A
  include MA
end
puts A.instance_variable_get(:@from_hook).inspect   # :ma
puts A.ancestors.include?(MA)                       # true

# (2) prepended — body-form.
module MB
  def self.prepended(base)
    puts "MB.prepended(#{base.name})"
  end
end
class B
  prepend MB
end
puts B.ancestors.first == MB                        # true (prepend wins)

# (3) Explicit-receiver form fires too.
module MC
  def self.included(base)
    puts "MC.included(#{base.name})"
  end
end
class C; end
C.include(MC)
puts C.ancestors.include?(MC)                       # true

# (4) CRuby fires the hook on EVERY include call, even when the
# chain insertion is a no-op (idempotent re-include only skips
# the chain mutation, not the callback).
module MD
  def self.included(base)
    puts "MD.included(#{base.name})"
  end
end
class D
  include MD
  include MD                                        # fires again
end
D.include(MD)                                       # fires a third time
puts D.ancestors.count(MD)                          # 1 — chain stays idempotent

# (5) Hook receiver is the module — sees self correctly.
module ME
  def self.included(base)
    puts "self == ME : #{self == ME}"
    puts "base == E  : #{base == E}"
  end
end
class E
  include ME
end

# (6) Subclass inherits the included module — no re-fire of the
# parent's included hook just because the subclass exists.
module MG
  def self.included(base)
    puts "MG.included(#{base.name})"
  end
end
class GParent
  include MG
end
class GChild < GParent
  # No `include MG` here; MG.included must NOT fire again.
end
puts GChild.ancestors.include?(MG)                  # true (inherited)

# (7) Multi-arg include — CRuby processes args RIGHT-to-LEFT,
# so M1 (leftmost) ends up at the head of ancestors and its
# `included` hook fires LAST.
module MF1
  def self.included(base); puts "MF1.included(#{base.name})"; end
end
module MF2
  def self.included(base); puts "MF2.included(#{base.name})"; end
end
class F
  include MF1, MF2
end
puts F.ancestors[1] == MF1            # true — MF1 ends up at the head

# (8) include + prepend on the same module — both succeed, both
# hooks fire. Per-chain idempotency means the include slot and
# the prepend slot are distinct insertions.
module MH
  def self.included(base);  puts "MH.included(#{base.name})";  end
  def self.prepended(base); puts "MH.prepended(#{base.name})"; end
end
class H
  include MH
  prepend MH
end
puts H.ancestors.first == MH                          # true (prepend slot)
puts H.ancestors.include?(MH)                         # true

# (9) Transitive include-chain idempotency still works:
# `include ContainsM; include M` skips the second include
# because M is reachable via ContainsM's include chain.
module TI_M; end
module TI_Contains; include TI_M; end
class TI_T
  include TI_Contains
  include TI_M                                        # no-op
end
puts TI_T.ancestors[1] == TI_Contains                 # true
puts TI_T.ancestors[2] == TI_M                        # true

# (10) No hooks defined — silent no-op (CRuby doesn't raise).
module MI
  # No included/prepended override.
end
class I
  include MI
end
puts I.ancestors.include?(MI)                       # true
