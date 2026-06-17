# Bare `extend Mod` (implicit self) inside an instance method — dispatches on
# self, like `self.extend(Mod)`. Surfaced by sequel's Database#adapter_initialize
# (`extend UnmodifiedIdentifiers::DatabaseMethods`).
module Greet
  def hi; "hi from #{label}"; end
end
module Label
  def label; "M"; end
end
class C
  def setup; extend(Greet); extend(Label); end
end
c = C.new
p c.respond_to?(:hi)   # false (not extended yet)
c.setup
p c.hi                 # "hi from M"
p c.singleton_class.include?(Greet)  # true
# (`c.is_a?(Greet)` after extend is a separate is_a?-vs-extend gap, not the
# bare-call routing this fixture covers — omitted.)

# bare extend returns self
class D
  def chain; extend(Greet).equal?(self); end
end
p D.new.chain          # true
