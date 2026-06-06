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

  # Note: CRuby's `Set#hash` returns an order-independent integer
  # so two sets with the same contents hash equally. We'd
  # implement it as XOR over `k.hash` for each element, but
  # Tier 1 Integer / Symbol don't carry `.hash` yet — only
  # `String#hash` is wired today. Leaving `hash` out means
  # equal Sets still report `Set#==` true (content walk through
  # `include?`); only `Hash` keyed by Set instances would
  # diverge, which isn't a shape gem helpers exercise.

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

  def inspect
    if empty?
      "#<Set: {}>"
    else
      parts = @hash.keys.map { |k| k.inspect }
      "#<Set: {" + parts.join(", ") + "}>"
    end
  end
  alias_method :to_s, :inspect
end
