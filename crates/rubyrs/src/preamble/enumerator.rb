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

  # Two construction forms:
  #   - `Enumerator.new(size = nil) { |y| y << 1; y.yield(a, b) }` —
  #     generator/yielder block; the optional leading arg is the declared
  #     `size` (returned by #size; nil when not given).
  #   - `Enumerator.new(obj, meth, args)` — the `enum_for` form (`args`
  #     is the already-collected Array, passed not splatted to avoid
  #     local-splat). Distinguished by whether a block was given.
  def initialize(obj = nil, meth = :each, args = [], &block)
    if block
      @gen = block
      @size = obj # the optional leading `size` arg (nil if absent)
    else
      @obj = obj
      @meth = meth
      @args = args
    end
  end

  # Drive the enumerator with the iteration block. Without a block, an
  # Enumerator is its own enumerator (CRuby returns self). The yielder
  # form runs the generator EAGERLY (each `y << v` / `y.yield(..)` calls
  # straight through to `block`).
  def each(&block)
    return self unless block
    if @gen
      @gen.call(Yielder.new(&block))
      self
    else
      case @args.length
      when 0 then @obj.__send__(@meth, &block)
      when 1 then @obj.__send__(@meth, @args[0], &block)
      when 2 then @obj.__send__(@meth, @args[0], @args[1], &block)
      else        @obj.__send__(@meth, @args[0], @args[1], @args[2], &block)
      end
    end
  end

  # `size` — the number of elements without iterating, or nil if not
  # known. The generator form reports the size declared at construction
  # (`Enumerator.new(n) { }`), nil otherwise. The enum_for form has no
  # declared size, so it counts via `to_a` — exact for the finite
  # collections rubyrs enumerates (each/map/select/each_slice all match
  # CRuby's result), at the cost of one materialization.
  def size
    return @size if @gen
    to_a.size
  end

  # `next` / `peek` / `rewind` — external iteration. CRuby drives the
  # source lazily through a Fiber; rubyrs has no lazy Enumerator (the
  # generator form already runs eagerly), so on first use the whole
  # enumeration is materialized into a buffer and walked with a cursor.
  # Finite enumerators behave identically to CRuby (incl. StopIteration
  # at the end, which `loop` rescues); an infinite generator would hang
  # here — the same limitation as the eager generator form.
  def next
    __materialize
    raise StopIteration, "iteration reached an end" if @cursor >= @buffer.length
    value = @buffer[@cursor]
    @cursor += 1
    value
  end

  def peek
    __materialize
    raise StopIteration, "iteration reached an end" if @cursor >= @buffer.length
    @buffer[@cursor]
  end

  # Restart external iteration. The buffer is dropped so the next
  # `next`/`peek` re-drives the source from the beginning (matching
  # CRuby, where rewind re-runs the enumeration).
  def rewind
    @buffer = nil
    @cursor = 0
    self
  end

  def __materialize
    return if @buffer
    @buffer = to_a
    @cursor = 0
  end
  private :__materialize

  # `enum.lazy` — wrap this Enumerator in a lazy chain.
  def lazy
    Enumerator::Lazy.new(self)
  end

  # `Enumerator::Yielder` — the object handed to a generator block. `<<`
  # and `yield` forward straight to the consumer's iteration block.
  class Yielder
    def initialize(&block)
      @block = block
    end

    def <<(value)
      @block.call(value)
      self
    end

    def yield(*args)
      @block.call(*args)
    end
  end

  # `Enumerator::Lazy` — a deferred chain of element transforms over a
  # source that responds to `each`. CRuby drives the source lazily via a
  # Fiber; rubyrs builds the chain as nested closures (a transducer-style
  # pipeline) and walks the source ONE element at a time when forced,
  # short-circuiting with `throw` so `take` / `first` never over-iterate.
  # This is what makes infinite sources work — `(1..Float::INFINITY)
  # .lazy.map { ... }.select { ... }.first(5)` — given the endless-range
  # `each` primitive.
  #
  # Each lazy operation returns a NEW Lazy with the op appended; nothing
  # runs until a forcing method (`first` / `to_a` / `force`, inherited
  # from Enumerator, which drive `each`). Stateful stages (take / drop /
  # drop_while / with_index) capture fresh counters per `each` call.
  class Lazy < Enumerator
    def initialize(source)
      @source = source
      @ops = []
    end

    # Append an op, returning a fresh Lazy that shares the source.
    def __chain(op)
      l = Lazy.new(@source)
      l.instance_variable_set(:@ops, @ops + [op])
      l
    end
    private :__chain

    def map(&block); __chain([:map, block]); end
    alias_method :collect, :map
    def flat_map(&block); __chain([:flat_map, block]); end
    alias_method :collect_concat, :flat_map
    def select(&block); __chain([:select, block]); end
    alias_method :filter, :select
    def reject(&block); __chain([:reject, block]); end
    def filter_map(&block); __chain([:filter_map, block]); end
    def take_while(&block); __chain([:take_while, block]); end
    def drop_while(&block); __chain([:drop_while, block]); end
    def take(n); __chain([:take, n]); end
    def drop(n); __chain([:drop, n]); end
    def with_index(offset = 0); __chain([:with_index, offset]); end

    # Drive the source through the op pipeline. The pipeline is built
    # inside-out: the consumer block is the innermost stage, each op
    # wraps the stage downstream of it (so ops apply in declaration
    # order). `throw(:__lazy_stop)` from a `take`/`take_while` stage
    # unwinds out of the source's `each`.
    def each(&consumer)
      return self unless consumer
      pipeline = consumer
      @ops.reverse_each do |op|
        pipeline = __stage(op, pipeline)
      end
      catch(:__lazy_stop) do
        @source.each { |*x| pipeline.call(__lazy_one(x)) }
      end
      self
    end

    def __lazy_one(x)
      x.length == 1 ? x[0] : x
    end
    private :__lazy_one

    # Build one pipeline stage: a proc that receives an upstream element
    # and calls `downstream` zero or more times (filtering, mapping,
    # flattening, or stopping the whole walk).
    def __stage(op, downstream)
      case op[0]
      when :map
        f = op[1]
        proc { |x| downstream.call(f.call(x)) }
      when :select
        pred = op[1]
        proc { |x| downstream.call(x) if pred.call(x) }
      when :reject
        pred = op[1]
        proc { |x| downstream.call(x) unless pred.call(x) }
      when :filter_map
        f = op[1]
        proc { |x| y = f.call(x); downstream.call(y) if y }
      when :flat_map
        f = op[1]
        proc do |x|
          r = f.call(x)
          if r.is_a?(Array)
            r.each { |e| downstream.call(e) }
          else
            downstream.call(r)
          end
        end
      when :take_while
        pred = op[1]
        proc { |x| pred.call(x) ? downstream.call(x) : throw(:__lazy_stop) }
      when :drop_while
        pred = op[1]
        dropping = true
        proc do |x|
          dropping = false if dropping && !pred.call(x)
          downstream.call(x) unless dropping
        end
      when :take
        n = op[1]
        count = 0
        proc do |x|
          throw(:__lazy_stop) if count >= n
          downstream.call(x)
          count += 1
          throw(:__lazy_stop) if count >= n
        end
      when :drop
        n = op[1]
        seen = 0
        proc do |x|
          if seen >= n
            downstream.call(x)
          else
            seen += 1
          end
        end
      when :with_index
        i = op[1]
        proc { |x| downstream.call([x, i]); i += 1 }
      end
    end
    private :__stage

    # `force` is the CRuby alias for `to_a` (inherited from Enumerator,
    # which drives `each`). `lazy` on a Lazy is a no-op (returns self).
    def force
      to_a
    end

    def lazy
      self
    end
  end

  # Collapse a `|*x|`-captured arg list to a single value (most
  # enumerators yield one value) or keep the Array (multi-value sources
  # like a Hash#each_pair enumerator yield pairs).
  def __enum_one(x)
    x.length == 1 ? x[0] : x
  end
  private :__enum_one

  def map
    return enum_for(:map) unless block_given?
    result = []
    each { |*x| result << yield(*x) }
    result
  end
  alias_method :collect, :map

  def to_a
    result = []
    each { |*x| result << __enum_one(x) }
    result
  end
  alias_method :entries, :to_a

  # `enum.to_h` — collect `[k, v]` pairs into a Hash. Without a block
  # each yielded value must already be a pair; with a block, the block
  # maps each element to a pair. Mirrors Array#to_h's error wording.
  def to_h
    result = {}
    has_block = block_given?
    each do |*x|
      pair = has_block ? yield(*x) : __enum_one(x)
      unless pair.is_a?(Array)
        raise TypeError, "wrong element type #{pair.class} (expected array)"
      end
      unless pair.length == 2
        raise ArgumentError, "element has wrong array length (expected 2, was #{pair.length})"
      end
      result[pair[0]] = pair[1]
    end
    result
  end
  alias_method :force, :to_a

  def select
    return enum_for(:select) unless block_given?
    result = []
    each { |*x| result << __enum_one(x) if yield(*x) }
    result
  end
  alias_method :filter, :select

  def reject
    return enum_for(:reject) unless block_given?
    result = []
    each { |*x| result << __enum_one(x) unless yield(*x) }
    result
  end

  def with_index(offset = 0)
    # No-block form must carry the offset into the returned Enumerator
    # (`e.with_index(1).to_a`); a bare `enum_for(:with_index)` would
    # drop it and restart numbering at 0.
    return enum_for(:with_index, offset) unless block_given?
    i = offset
    # The block handed to `each` must return the USER block's value, not
    # the `i += 1` increment — otherwise the underlying method
    # (`map`/`select`/`sort_by`/…) collects/filters on the counter
    # instead of the real result (`e.map.with_index { |x, i| ... }`).
    each do |*x|
      result = yield(__enum_one(x), i)
      i += 1
      result
    end
  end
  alias_method :each_with_index, :with_index

  def with_object(memo)
    each { |*x| yield(__enum_one(x), memo) }
    memo
  end
  alias_method :each_with_object, :with_object

  def count
    n = 0
    each { |*x| n += 1 }
    n
  end

  def first(n = nil)
    result = []
    take = n || 1
    # `throw`, not `break`: the generator (yielder) form drives `each`
    # from inside a separate proc, and `break` can't cross that proc
    # boundary (LocalJumpError). `throw`/`catch` is a non-local exit
    # that unwinds through it, stopping the eager generator early.
    if take > 0
      catch(:__enum_first) do
        each do |*x|
          result << __enum_one(x)
          throw(:__enum_first) if result.length >= take
        end
      end
    end
    n.nil? ? result[0] : result
  end

  def include?(obj)
    each { |*x| return true if __enum_one(x) == obj }
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

# `enum.lazy` lives on Enumerator; the collection types get it by
# reopening (rubyrs's Enumerable module is a stub, so it can't carry the
# shared method). Each just wraps `self` — a source that responds to
# `each` — in a lazy chain. Range's `each` covers endless / infinite
# bounds, so `(1..).lazy` / `(1..Float::INFINITY).lazy` work.
class Array
  def lazy; Enumerator::Lazy.new(self); end
end
class Hash
  def lazy; Enumerator::Lazy.new(self); end
end
class Range
  def lazy; Enumerator::Lazy.new(self); end
end
