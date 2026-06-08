# Minimal Enumerator + Kernel#enum_for / #to_enum.
#
# rubyrs has no native Enumerator; this models the
# `enum_for(:meth, *args)` form — an object that, when iterated,
# re-invokes `recv.meth(*args)` with the iteration block. (The
# `Enumerator.new { |y| ... }` yielder form is NOT modelled.) This is
# what rouge's lexer/formatter pipeline needs: `lex(string)` without a
# block returns `enum_for(:lex, string)`, later driven via
# `tokens.each { |tok, val| ... }`.
#
# NOTE: rubyrs's compiler rejects local-variable splat in a call /
# `yield(*x)`, so this file is written WITHOUT either — captured args
# are stored as an Array and re-applied through explicit small-arity
# dispatch; the Enumerable helpers use single-value block params.
# rubyrs's `Enumerable` module is an empty stub, so the common methods
# are defined directly on Enumerator in terms of #each.
class Enumerator
  include Enumerable

  # `args` is the already-collected Array of extra arguments (the
  # caller passes the Array, not a splat, to avoid local-splat).
  def initialize(obj = nil, meth = :each, args = [])
    @obj = obj
    @meth = meth
    @args = args
  end

  # Re-invoke the captured method with the iteration block. Without a
  # block, an Enumerator is its own enumerator (CRuby returns self).
  def each(&block)
    return self unless block
    case @args.length
    when 0 then @obj.__send__(@meth, &block)
    when 1 then @obj.__send__(@meth, @args[0], &block)
    when 2 then @obj.__send__(@meth, @args[0], @args[1], &block)
    else        @obj.__send__(@meth, @args[0], @args[1], @args[2], &block)
    end
  end

  def map
    return enum_for(:map) unless block_given?
    result = []
    each { |x| result << yield(x) }
    result
  end
  alias_method :collect, :map

  def to_a
    result = []
    each { |x| result << x }
    result
  end
  alias_method :entries, :to_a
  alias_method :force, :to_a

  def select
    return enum_for(:select) unless block_given?
    result = []
    each { |x| result << x if yield(x) }
    result
  end
  alias_method :filter, :select

  def reject
    return enum_for(:reject) unless block_given?
    result = []
    each { |x| result << x unless yield(x) }
    result
  end

  def with_index(offset = 0)
    return enum_for(:with_index) unless block_given?
    i = offset
    each do |x|
      yield(x, i)
      i += 1
    end
  end
  alias_method :each_with_index, :with_index

  def with_object(memo)
    each { |x| yield(x, memo) }
    memo
  end
  alias_method :each_with_object, :with_object

  def count
    n = 0
    each { |x| n += 1 }
    n
  end

  def first(n = nil)
    result = []
    take = n || 1
    each do |x|
      break if result.length >= take
      result << x
    end
    n.nil? ? result[0] : result
  end

  def include?(obj)
    each { |x| return true if x == obj }
    false
  end
end

module Kernel
  # `enum_for(:meth = :each, *args)` / `to_enum` — capture a deferred
  # iteration. Classic guard: `return enum_for(:meth, args) unless
  # block_given?` at the top of an iterator. `args` is collected via
  # the rest param and handed to Enumerator as a single Array.
  def enum_for(meth = :each, *args)
    Enumerator.new(self, meth, args)
  end
  alias_method :to_enum, :enum_for
end
