# Object#freeze for user-class instances. CRuby's freeze flips
# a per-object flag that `frozen?` reads; mutation surfaces
# FrozenError. rubyrs's implementation wires up the read/write
# surface against a per-Instance Cell<bool>. Full mutation
# guards (FrozenError on ivar set, singleton method install)
# are deferred — adding the freeze/frozen? read/write is what
# unblocks gem patterns like `EmptyMapping.new.freeze` on
# construction.

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

# 8. Reading from a frozen instance still works — only
# mutation is blocked (CRuby raises FrozenError; rubyrs leaves
# the mutation guard as deferred work and only wires the
# freeze/frozen? read/write surface). Ivar / attr reader
# remain unaffected. The fixture exercises read-only paths;
# adding a mutation-blocks-with-FrozenError scenario would
# trigger the documented divergence.
foo2 = Foo.new(7).freeze
puts "read_attr=#{foo2.x}"
puts "ivar_read=#{foo2.instance_variable_get(:@x)}"
