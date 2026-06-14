# `methods(false)` / `singleton_methods(false)` — the optional regular/all
# boolean restricts to the receiver's own methods. Sorted on both sides
# because CRuby returns definition order while rubyrs returns sorted.
module Foo
  def self.alpha; end
  def self.beta; end
end
p Foo.methods(false).sort
p Foo.singleton_methods(false).sort

obj = Object.new
def obj.only_me; end
p obj.methods(false).sort
p obj.singleton_methods(false).sort

# Bare form inside a module body (no explicit receiver).
module Bar
  def self.gamma; end
  RESULT = methods(false).sort
end
p Bar::RESULT
