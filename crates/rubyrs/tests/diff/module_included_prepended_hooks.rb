# Module.included(base) / Module.prepended(base) hooks — fire
# when a module is freshly inserted into a class's include /
# prepend chain. Receiver of the hook is the module being
# included/prepended; argument is the target class. Idempotent
# re-include must NOT re-fire. ActiveSupport::Concern, Rails
# plugin systems, and Sinatra extensions all hinge on this hook.

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

# (7) No hooks defined — silent no-op (CRuby doesn't raise).
module MI
  # No included/prepended override.
end
class I
  include MI
end
puts I.ancestors.include?(MI)                       # true
