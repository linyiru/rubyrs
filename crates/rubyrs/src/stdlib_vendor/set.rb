# Tier 3 pure-Ruby Set — subset matched to CRuby's
# stdlib/set.rb for the deterministic, Hash-backed core
# (add / remove / membership / size / iteration / set algebra).
# Methods that depend on stdlib niceties we don't model
# (SortedSet, RestrictedSet, Comparable mixin against Set,
# Marshal hooks, etc.) are NOT included; scripts that reach
# them get NoMethodError, which is the right "feature absent"
# surface for the embedding case.
#
# Gated behind the `stdlib` Cargo feature (see ADR 0017 row 125
# and the feature description in `crates/rubyrs/Cargo.toml`).
# Default builds do NOT include this file's behaviour.

class Set
  # `Set[1, 2, 3]` — class-method constructor, equivalent to
  # `Set.new([1, 2, 3])`. `Set[]` is the empty set.
  def self.[](*items)
    new(items)
  end

  def initialize(enum = nil)
    @hash = {}
    return if enum.nil?
    enum.each { |o| add(o) }
  end

  def add(o)
    @hash[o] = true
    self
  end
  alias_method :<<, :add

  def delete(o)
    @hash.delete(o)
    self
  end

  # `add?` / `delete?` return self on a real change, nil otherwise
  # (CRuby) — the idiomatic "insert/remove and tell me if it did
  # anything" form.
  def add?(o)
    return nil if include?(o)
    add(o)
  end

  def delete?(o)
    return nil unless include?(o)
    delete(o)
  end

  def clear
    @hash = {}
    self
  end

  def include?(o)
    @hash.key?(o)
  end
  alias_method :member?, :include?

  def size
    @hash.size
  end
  alias_method :length, :size

  def empty?
    @hash.empty?
  end

  def to_a
    @hash.keys
  end

  def each(&block)
    @hash.each_key(&block)
    self
  end

  def ==(other)
    return false unless other.is_a?(Set)
    return false unless size == other.size
    @hash.each_key { |k| return false unless other.include?(k) }
    true
  end
  alias_method :eql?, :==

  # `Set#hash` is order-independent — two sets with equal contents
  # hash equally — so Sets can be used as Hash keys. The backing
  # Hash's own `#hash` already folds its keys order-independently.
  def hash
    @hash.hash
  end

  # Union — every element from self, plus every element from
  # enum that wasn't already there.
  def |(enum)
    result = Set.new(self)
    enum.each { |o| result.add(o) }
    result
  end
  alias_method :+, :|
  alias_method :union, :|

  # Merge the elements of each enumerable into self (in place),
  # returning self. Accepts multiple enums (Ruby 3.x). Used by
  # Liquid's strainer.rb `add_filter` to fold a filter module's
  # public instance methods into the global filter set.
  def merge(*enums)
    enums.each { |enum| enum.each { |o| add(o) } }
    self
  end

  # Replace the contents with the elements of `enum` (CRuby
  # Set#replace). Returns self.
  def replace(enum)
    clear
    merge(enum)
    self
  end

  # Difference — elements in self that aren't in enum.
  def -(enum)
    result = Set.new
    drop = {}
    enum.each { |o| drop[o] = true }
    @hash.each_key { |k| result.add(k) unless drop.key?(k) }
    result
  end
  alias_method :difference, :-

  # Intersection — elements in both.
  def &(enum)
    result = Set.new
    keep = {}
    enum.each { |o| keep[o] = true }
    @hash.each_key { |k| result.add(k) if keep.key?(k) }
    result
  end
  alias_method :intersection, :&

  # Symmetric difference — elements in exactly one of the two sets.
  # Mirrors CRuby's algorithm (seed from enum, then toggle self's
  # elements) so the insertion ORDER — visible via inspect/to_a —
  # matches: `Set[1,2] ^ Set[2,3]` is `#<Set: {3, 1}>`.
  def ^(enum)
    n = Set.new
    enum.each { |o| n.add(o) }
    each { |o| n.include?(o) ? n.delete(o) : n.add(o) }
    n
  end

  # True when the two sets share no element.
  def disjoint?(other)
    other.each { |o| return false if include?(o) }
    true
  end

  # True when the two sets share at least one element.
  def intersect?(other)
    !disjoint?(other)
  end

  def subset?(other)
    return false unless other.is_a?(Set)
    return false if size > other.size
    @hash.each_key { |k| return false unless other.include?(k) }
    true
  end
  alias_method :<=, :subset?

  def superset?(other)
    return false unless other.is_a?(Set)
    other.subset?(self)
  end
  alias_method :>=, :superset?

  # Proper (strict) subset / superset — contained AND not equal.
  # `<` / `>` are the strict operators (CRuby), distinct from the
  # `<=` / `>=` non-strict aliases above.
  def proper_subset?(other)
    return false unless other.is_a?(Set)
    size < other.size && subset?(other)
  end
  alias_method :<, :proper_subset?

  def proper_superset?(other)
    return false unless other.is_a?(Set)
    size > other.size && superset?(other)
  end
  alias_method :>, :proper_superset?

  def inspect
    if empty?
      "#<Set: {}>"
    else
      parts = @hash.keys.map { |k| k.inspect }
      "#<Set: {" + parts.join(", ") + "}>"
    end
  end
  alias_method :to_s, :inspect

  # --- Enumerable surface ---
  #
  # CRuby's `Set` mixes in `Enumerable`, whose methods are all built
  # on `#each`. rubyrs's `Enumerable` stub adds nothing (the generic
  # over-#each implementation is a separate follow-up), so the Set
  # veneer carries the common slice directly. Each delegates to the
  # insertion-ordered element array (`to_a` == `@hash.keys`), so the
  # iteration order and per-element results inherit Array's
  # CRuby-parity behaviour. Discovery: P3 Jekyll spike —
  # `plugin.rb#descendants` does `@children.map(&:descendants)` then
  # `Set.new(out).flatten`.

  def map(&block)
    to_a.map(&block)
  end
  alias_method :collect, :map

  # In-place map: replace every element with the block's result.
  # Returns self (CRuby Set#collect! / #map!).
  def collect!
    return enum_for(:collect!) unless block_given?
    new_hash = {}
    to_a.each { |o| new_hash[yield(o)] = true }
    @hash = new_hash
    self
  end
  alias_method :map!, :collect!

  def flat_map(&block)
    to_a.flat_map(&block)
  end
  alias_method :collect_concat, :flat_map

  # Enumerable#filter_map (map + compact in one pass). Surfaced by
  # bridgetown-core's `configure_component_paths`
  # (`@components_load_paths.filter_map { … }`).
  def filter_map(&block)
    return to_a.filter_map unless block_given?
    to_a.filter_map(&block)
  end

  def select(&block)
    to_a.select(&block)
  end
  alias_method :filter, :select

  def reject(&block)
    to_a.reject(&block)
  end

  def find(&block)
    to_a.find(&block)
  end
  alias_method :detect, :find

  def each_with_object(memo, &block)
    to_a.each_with_object(memo, &block)
  end

  def each_with_index(&block)
    # Enumerable#each_with_index returns the receiver (the Set), not
    # the intermediate Array `to_a` yields.
    return to_a.each_with_index unless block_given?
    to_a.each_with_index(&block)
    self
  end

  def inject(*args, &block)
    to_a.inject(*args, &block)
  end
  alias_method :reduce, :inject

  def any?(&block)
    to_a.any?(&block)
  end

  def all?(&block)
    to_a.all?(&block)
  end

  def none?(&block)
    to_a.none?(&block)
  end

  def count(*args, &block)
    to_a.count(*args, &block)
  end

  def sum(*args, &block)
    to_a.sum(*args, &block)
  end

  def min(&block)
    to_a.min(&block)
  end

  def max(&block)
    to_a.max(&block)
  end

  def sort(&block)
    to_a.sort(&block)
  end

  def sort_by(&block)
    to_a.sort_by(&block)
  end

  def min_by(&block)
    to_a.min_by(&block)
  end

  def max_by(&block)
    to_a.max_by(&block)
  end

  def group_by(&block)
    to_a.group_by(&block)
  end

  def partition(&block)
    to_a.partition(&block)
  end

  def first(*args)
    to_a.first(*args)
  end

  def to_set
    self
  end

  # `Set#flatten` (Set-specific, not Enumerable): recursively merge
  # any nested Sets into a single new Set. Non-Set members are added
  # as-is.
  def flatten
    result = self.class.new
    each do |item|
      if item.is_a?(Set)
        item.flatten.each { |e| result.add(e) }
      else
        result.add(item)
      end
    end
    result
  end
end

# CRuby's `set` stdlib adds `#to_set` to Enumerable, so every
# Array/Hash/Range gains it once `set` is required. rubyrs's
# Enumerable mixin doesn't propagate methods to includers, so wire
# the common collections directly. Discovery: P3 Jekyll spike —
# `cleaner.rb#keep_dirs` does `dirs.to_set`.
class Array
  def to_set
    Set.new(self)
  end
end

class Hash
  def to_set
    Set.new(to_a)
  end
end

class Range
  def to_set
    Set.new(to_a)
  end
end
