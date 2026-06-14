# Object#freeze for user-class instances. CRuby's freeze flips
# a per-object flag that `frozen?` reads; mutation surfaces
# FrozenError. rubyrs wires the read/write surface against a
# per-Instance Cell<bool>, AND every instance-variable write
# (`@x = v` / `@x += 1` / instance_variable_set) raises
# FrozenError on a frozen receiver — rack Builder#freeze_app
# freezes the app + middleware, so a frozen handler that sets
# `@x` during a request must 500.

class Foo
  def initialize(x); @x = x; end
  attr_reader :x
end

# 1. respond_to? — feature detection. Pre-fix this returned
# false because `freeze` wasn't in the universal method-name
# whitelist that `respond_to?` consults.
foo = Foo.new(1)
puts "respond_to_freeze=#{foo.respond_to?(:freeze)}"
puts "respond_to_frozen?=#{foo.respond_to?(:frozen?)}"

# 2. Initial state — fresh instance is not frozen.
puts "fresh_frozen=#{foo.frozen?}"

# 3. freeze returns self, flips the flag.
ret = foo.freeze
puts "freeze_returns_self=#{ret.equal?(foo)}"
puts "after_freeze_frozen=#{foo.frozen?}"

# 4. freeze is idempotent — calling again stays frozen,
# returns self.
foo.freeze
puts "still_frozen=#{foo.frozen?}"

# 5. The flag is per-object — freezing one instance doesn't
# freeze others of the same class.
bar = Foo.new(2)
puts "sibling_frozen=#{bar.frozen?}"
puts "sibling_after=#{bar.x}"

# 6. Immediates / nil / bool are always frozen (CRuby parity).
puts "int_frozen=#{1.frozen?}"
puts "sym_frozen=#{:x.frozen?}"
puts "true_frozen=#{true.frozen?}"
puts "false_frozen=#{false.frozen?}"
puts "nil_frozen=#{nil.frozen?}"

# 7. Method chain — `Foo.new.freeze.frozen?` is the gem-idiom
# shape (PR #374's Tilt::EmptyMapping.new.freeze).
puts "chain=#{Foo.new(99).freeze.frozen?}"

# 8. Reading from a frozen instance still works.
foo2 = Foo.new(7).freeze
puts "read_attr=#{foo2.x}"
puts "ivar_read=#{foo2.instance_variable_get(:@x)}"

# 9. Mutation via `instance_variable_set` on a frozen
# instance raises FrozenError (CRuby parity). The freeze
# read/write surface was shipped earlier; this scenario
# locks in the matching mutation-guard.
foo3 = Foo.new(99).freeze
caught = nil
begin
  foo3.instance_variable_set(:@late, "set-after-freeze")
rescue FrozenError => e
  caught = e.message[/can't modify frozen \w+/]
end
puts "mut_blocked=#{caught.inspect}"

# 10. Plain ivar assignment (`@x = v` / StoreIvar) and the
# `@x += 1` IncIvar fast path also raise on a frozen receiver —
# the rack freeze_app shape (a method body setting `@a = 1`).
class Counter
  def initialize; @n = 0; end
  def set;  @a = 1;  end
  def bump; @n += 1; end
end
froz = Counter.new.freeze
m1 = (froz.set rescue $!.message[/can't modify frozen \w+/])
puts "store_ivar=#{m1.inspect}"
m2 = (froz.bump rescue $!.message[/can't modify frozen \w+/])
puts "inc_ivar=#{m2.inspect}"
# A non-frozen Counter mutates fine.
ok = Counter.new
ok.set; ok.bump
puts "unfrozen=#{ok.instance_variable_get(:@a)},#{ok.instance_variable_get(:@n)}"
