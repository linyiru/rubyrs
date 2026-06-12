# Three minitest-stub-family behaviours:
# 1. `super` with no superclass method falls back to
#    method_missing on self (CRuby: BasicObject's default then
#    raises, so no-mm programs are unaffected). Mock's
#    test_dynamic_method stubs a method that only "exists" via
#    `def self.method_missing`.
# 2. caller(Range) — minitest mock.rb's KW_WARNED path does
#    `caller(1..1).first`.
# 3. Bare (implicit-self) calls to the reflection universals
#    (methods/singleton_methods/...) — Object#stub calls
#    `methods.map(&:to_s)` with self = the stubbed object.

# -- 1. super -> method_missing --
dynamic = Class.new do
  def self.method_missing(meth, *args, &block)
    meth == :found ? [:mm_found, args] : super
  end
end
# (`|*args, **kwargs| super(*args, **kwargs)` works too, but the
# empty-kwargs `**{}` elision is a separate documented gap —
# SUBSET.md's SuperApply section — so the fixture pins the
# plain-splat shape.)
dynamic.singleton_class.send(:define_method, :found) do |*args|
  super(*args)
end
p dynamic.found(1, 2)

# Instance-level twin.
class MMHolder
  def method_missing(meth, *args)
    meth == :ghost ? [:imm, args] : super
  end
end
class MMChild < MMHolder
  def ghost(*args)
    super
  end
end
p MMChild.new.ghost(7)

# Without a user method_missing the original error survives.
class NoMM
  def lonely
    super
  end
end
begin
  NoMM.new.lonely
rescue NoMethodError => e
  puts e.message.tr("`", "'").sub(/0x[0-9a-f]+/, "0xXXX")
end

# -- 2. caller(Range) --
def lvl3
  [caller(1..1).length, caller(1...2).length, caller(2..).length >= 1,
   caller(..1).length, caller(3..1), caller(50..60)]
end
def lvl2; lvl3; end
def lvl1; lvl2; end
p lvl1

# -- 3. bare reflection universals --
class BareProbe
  def probe
    [methods.include?(:probe), singleton_methods, public_methods.class,
     private_methods.class, protected_methods.class,
     frozen?, object_id.class, hash.class, itself.equal?(self),
     singleton_class.class]
  end
end
p BareProbe.new.probe
