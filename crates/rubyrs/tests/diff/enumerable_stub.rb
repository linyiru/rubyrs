# `Enumerable` preamble stub — empty class so
# `class Foo; include Enumerable; ...; end` loads without
# raising "wrong argument type NilClass (expected Module)".
#
# CRuby's Enumerable defines ~50 derived methods (map, select,
# inject, ...) all in terms of `#each`. Our empty stub doesn't
# install those, so user classes that `include Enumerable` get
# the include accepted but don't gain the methods. The fixture
# covers the load-time round-trip — what actually unblocks
# rake/linked_list.rb in the try-run set.

# Load-time include succeeds.
class MyList
  include Enumerable

  def initialize(*items)
    @items = items
  end

  # The host class still provides its own `each`.
  def each(&block)
    @items.each(&block)
  end

  # Override `to_a` directly since the Enumerable-derived one
  # isn't installed.
  def to_a
    out = []
    @items.each { |x| out << x }
    out
  end
end

list = MyList.new(1, 2, 3)
puts list.to_a.inspect
list.each { |x| puts x }

# Multiple includers — the stub is shared across classes (same
# Rc<Class>) so re-including is idempotent on the chain.
class Other
  include Enumerable
end
puts Other.new.is_a?(Enumerable)

# Including alongside other modules (Comparable) — both stubs
# coexist in the include chain.
class Mixed
  include Enumerable
  include Comparable
  attr_reader :n
  def initialize(n); @n = n; end
  def <=>(other); @n <=> other.n; end
end
a = Mixed.new(1)
b = Mixed.new(2)
puts(a < b)
puts a.is_a?(Enumerable)
puts a.is_a?(Comparable)
