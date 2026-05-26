# Class-level instance variables (`@foo` on a `Value::Class`).
# Previously a documented divergence: writes from class methods
# didn't persist, attr_accessor on `class << self` returned nil
# forever. This PR adds an `ivars` table to the Class struct
# and routes the Op::LoadIvar / Op::StoreIvar / Op::IncIvar
# handlers based on whether `self` is Object vs Class.
#
# Tilt's `module Tilt; @default_mapping = ...; class << self;
# attr_reader :default_mapping; end; end` round-trips after
# this PR.

# --- Plain class method round-trip ---
class Counter
  @n = 0
  def self.inc; @n += 1; end
  def self.value; @n; end
end
puts Counter.value   # 0
Counter.inc
Counter.inc
Counter.inc
puts Counter.value   # 3

# --- attr_accessor in `class << self` body ---
class Foo
  class << self
    attr_accessor :label
    attr_reader :version
  end
end
puts Foo.label.inspect    # nil
Foo.label = "rubyrs"
puts Foo.label            # rubyrs
puts Foo.version.inspect  # nil (never written)

# --- Module-level (Tilt-shaped) ---
module Tilt
  @default_mapping = "the mapping"
  @extract_fixed_locals = false
  class << self
    attr_reader :default_mapping
    attr_accessor :extract_fixed_locals
  end
end
puts Tilt.default_mapping       # the mapping
puts Tilt.extract_fixed_locals  # false
Tilt.extract_fixed_locals = true
puts Tilt.extract_fixed_locals  # true

# --- NOT inherited: each class has its own slot ---
class Base
  @marker = "base"
  def self.marker; @marker; end
end
class Child < Base
  @marker = "child"
end
puts Base.marker   # base
puts Child.marker  # child (NOT "base" — class ivars don't inherit)

# --- Op::IncIvar / IncIvarNoPush fast path on Class self ---
class Tally
  @hits = 0
  def self.bump!; @hits += 1; end
  def self.hits; @hits; end
end
5.times { Tally.bump! }
puts Tally.hits   # 5

# --- Heap-backed value survives GC across calls ---
class Bag
  @items = []
  def self.add(x); @items << x; end
  def self.items; @items; end
end
Bag.add("a")
Bag.add("b")
Bag.add("c")
# Force allocations to trigger GC; @items must survive.
1000.times { |i| "x#{i}" }
puts Bag.items.inspect  # ["a", "b", "c"]
