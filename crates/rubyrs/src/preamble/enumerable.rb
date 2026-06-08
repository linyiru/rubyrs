# Enumerable — stub class (we don't have real Modules in this
# subset). CRuby's Enumerable defines ~50 methods
# (each_with_index, map, select, reject, inject, sort, to_a, ...)
# all in terms of a host class's `#each`. For built-in
# collections (Array/Hash/Range), iteration methods are wired in
# `vm/iter.rs`'s block-dispatch paths, not via Enumerable
# include; for user classes the host provides `def each`
# directly. Either way, the Enumerable-derived methods aren't
# automatically gained through an empty stub.
#
# Why keep the stub anyway: `class Foo; include Enumerable; def
# each; ...; end; end` (commonly executed while loading a class
# body, but also supported at arbitrary runtime points and via
# the explicit `Foo.include(Enumerable)` form) pushes Enumerable
# onto Foo's `includes` chain (vm/dispatch.rs's include arm;
# lookup walks the chain at method-dispatch time, no copy).
# Empty Enumerable adds nothing to dispatch but doesn't crash.
# Before this stub, `include Enumerable` raised "wrong argument
# type NilClass (expected Module)" and the file failed to load.
# Affected: rake/linked_list.rb at minimum (Plan A try-run
# target), plus any other codebase that does the same
# `include Enumerable + def each` pattern. Methods like `.map`
# on a user `LinkedList` instance still NoMethodError at call
# time — documented divergence, follow-up PR.

# Defined as a `module` (not `class`) so `is_module` is true:
# CRuby's `Enumerable` is a Module, and `Mod.include?(Enumerable)` /
# `Class#include Enumerable` validate the argument is a Module
# (`wrong argument type Class (expected Module)` otherwise).
# Discovery: P3 Jekyll spike — liquid `Drop.invokable_methods` does
# `include?(Enumerable)`.
#
# The methods below are the canonical Enumerable surface, all defined in
# terms of the includer's `#each` (exactly how CRuby builds them). A
# class that does `include Enumerable; def each; ...; end` thus gains the
# whole API for free — what liquid's `InputIterator` (include Enumerable +
# def each) needs for `to_a`/`map`/`sort`/`where`, and what countless gems
# rely on. Multi-value yields collapse like CRuby: 0 args → nil, 1 → the
# value, 2+ → an Array (`__enum_elem`).
module Enumerable
  # Collapse a `|*x|`-captured yield to a single Enumerable element.
  def __enum_elem(x)
    x.length <= 1 ? x[0] : x
  end
  private :__enum_elem

  def to_a
    result = []
    each { |*x| result << __enum_elem(x) }
    result
  end
  alias_method :entries, :to_a

  def map
    return to_enum(:map) unless block_given?
    result = []
    each { |*x| result << yield(*x) }
    result
  end
  alias_method :collect, :map

  def flat_map
    return to_enum(:flat_map) unless block_given?
    result = []
    each do |*x|
      v = yield(*x)
      if v.is_a?(Array)
        v.each { |e| result << e }
      else
        result << v
      end
    end
    result
  end
  alias_method :collect_concat, :flat_map

  def select
    return to_enum(:select) unless block_given?
    result = []
    each { |*x| e = __enum_elem(x); result << e if yield(*x) }
    result
  end
  alias_method :filter, :select
  alias_method :find_all, :select

  def reject
    return to_enum(:reject) unless block_given?
    result = []
    each { |*x| e = __enum_elem(x); result << e unless yield(*x) }
    result
  end

  def filter_map
    return to_enum(:filter_map) unless block_given?
    result = []
    each { |*x| v = yield(*x); result << v if v }
    result
  end

  def find(ifnone = nil)
    return to_enum(:find, ifnone) unless block_given?
    each { |*x| e = __enum_elem(x); return e if yield(*x) }
    ifnone ? ifnone.call : nil
  end
  alias_method :detect, :find

  def find_index(target = (no_arg = true; nil))
    i = 0
    if no_arg
      each { |*x| return i if yield(*x); i += 1 }
    else
      each { |*x| return i if __enum_elem(x) == target; i += 1 }
    end
    nil
  end

  def each_with_index
    return to_enum(:each_with_index) unless block_given?
    i = 0
    each { |*x| yield(__enum_elem(x), i); i += 1 }
    self
  end

  def reverse_each(&block)
    return to_enum(:reverse_each) unless block_given?
    to_a.reverse_each(&block)
    self
  end

  def each_with_object(memo)
    return to_enum(:each_with_object, memo) unless block_given?
    each { |*x| yield(__enum_elem(x), memo) }
    memo
  end

  def reduce(*args)
    if args.length == 2
      memo = args[0]
      sym = args[1]
      each { |*x| memo = memo.send(sym, __enum_elem(x)) }
      memo
    elsif args.length == 1 && !block_given?
      sym = args[0]
      memo = (no_memo = true; nil)
      each do |*x|
        e = __enum_elem(x)
        if no_memo then memo = e; no_memo = false else memo = memo.send(sym, e) end
      end
      memo
    else
      memo = args.length == 1 ? args[0] : (no_memo = true; nil)
      each do |*x|
        e = __enum_elem(x)
        if no_memo then memo = e; no_memo = false else memo = yield(memo, e) end
      end
      memo
    end
  end
  alias_method :inject, :reduce

  def count(*args)
    n = 0
    if args.length == 1
      target = args[0]
      each { |*x| n += 1 if __enum_elem(x) == target }
    elsif block_given?
      each { |*x| n += 1 if yield(*x) }
    else
      each { n += 1 }
    end
    n
  end

  def sum(init = 0)
    memo = init
    if block_given?
      each { |*x| memo += yield(*x) }
    else
      each { |*x| memo += __enum_elem(x) }
    end
    memo
  end

  def min
    result = (none = true; nil)
    each do |*x|
      e = __enum_elem(x)
      if none then result = e; none = false
      elsif block_given? then result = e if yield(e, result) < 0
      elsif (e <=> result) < 0 then result = e end
    end
    result
  end

  def max
    result = (none = true; nil)
    each do |*x|
      e = __enum_elem(x)
      if none then result = e; none = false
      elsif block_given? then result = e if yield(e, result) > 0
      elsif (e <=> result) > 0 then result = e end
    end
    result
  end

  def min_by
    return to_enum(:min_by) unless block_given?
    best = nil; best_key = nil; none = true
    each do |*x|
      e = __enum_elem(x); k = yield(e)
      if none || (k <=> best_key) < 0 then best = e; best_key = k; none = false end
    end
    best
  end

  def max_by
    return to_enum(:max_by) unless block_given?
    best = nil; best_key = nil; none = true
    each do |*x|
      e = __enum_elem(x); k = yield(e)
      if none || (k <=> best_key) > 0 then best = e; best_key = k; none = false end
    end
    best
  end

  def sort(&block)
    to_a.sort(&block)
  end

  def sort_by(&block)
    return to_enum(:sort_by) unless block_given?
    to_a.map { |e| [yield(e), e] }.sort { |a, b| a[0] <=> b[0] }.map { |pair| pair[1] }
  end

  def group_by
    return to_enum(:group_by) unless block_given?
    result = {}
    each do |*x|
      e = __enum_elem(x); k = yield(e)
      (result[k] ||= []) << e
    end
    result
  end

  def partition
    return to_enum(:partition) unless block_given?
    yes = []; no = []
    each { |*x| e = __enum_elem(x); if yield(*x) then yes << e else no << e end }
    [yes, no]
  end

  def include?(obj)
    each { |*x| return true if __enum_elem(x) == obj }
    false
  end
  alias_method :member?, :include?

  def all?
    each do |*x|
      v = block_given? ? yield(*x) : __enum_elem(x)
      return false unless v
    end
    true
  end

  def any?
    each do |*x|
      v = block_given? ? yield(*x) : __enum_elem(x)
      return true if v
    end
    false
  end

  def none?
    each do |*x|
      v = block_given? ? yield(*x) : __enum_elem(x)
      return false if v
    end
    true
  end

  def one?
    n = 0
    each do |*x|
      v = block_given? ? yield(*x) : __enum_elem(x)
      n += 1 if v
      return false if n > 1
    end
    n == 1
  end

  def first(n = (one = true; 1))
    result = []
    return one ? nil : result if n <= 0
    each { |*x| result << __enum_elem(x); break if result.length >= n }
    one ? result[0] : result
  end

  def take(n)
    result = []
    return result if n <= 0
    each { |*x| result << __enum_elem(x); break if result.length >= n }
    result
  end

  def drop(n)
    result = []
    i = 0
    each { |*x| result << __enum_elem(x) if i >= n; i += 1 }
    result
  end

  def take_while
    return to_enum(:take_while) unless block_given?
    result = []
    each { |*x| break unless yield(*x); result << __enum_elem(x) }
    result
  end

  def drop_while
    return to_enum(:drop_while) unless block_given?
    result = []
    dropping = true
    each do |*x|
      e = __enum_elem(x)
      dropping = false if dropping && !yield(*x)
      result << e unless dropping
    end
    result
  end

  def to_h
    result = {}
    each do |*x|
      pair = block_given? ? yield(*x) : __enum_elem(x)
      result[pair[0]] = pair[1]
    end
    result
  end

  def tally
    result = {}
    each { |*x| e = __enum_elem(x); result[e] = (result[e] || 0) + 1 }
    result
  end

  def uniq
    seen = {}
    result = []
    each do |*x|
      e = __enum_elem(x)
      k = block_given? ? yield(e) : e
      unless seen.key?(k)
        seen[k] = true
        result << e
      end
    end
    result
  end

  def lazy
    to_enum(:each).lazy
  end
end
