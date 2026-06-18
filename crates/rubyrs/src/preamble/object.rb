# Universal ancestor hierarchy: BasicObject ← Object (Kernel
# is mixed into Object as a module, not a superclass between
# them). Mirrors CRuby's actual chain instead of an isolated
# Object stub. The resulting Object.ancestors is
# `[Object, Kernel, BasicObject]` — Kernel appears between
# Object and BasicObject in the ancestor *walk* because of
# the include, but it's not a superclass.
#
# Why model the full chain:
#   - `Object.ancestors` returns `[Object, Kernel, BasicObject]`,
#     matching CRuby — reflection-heavy code (e.g. modern DSLs
#     that walk `obj.class.ancestors`) sees the same shape.
#   - `Object < BasicObject` makes `Module#superclass` semantically
#     distinguishable: classes have a superclass chain, modules
#     don't — the dispatch arm can raise NoMethodError on
#     `module M; end; M.superclass` like CRuby does.
#   - Lays the groundwork for synthesising `Kernel.instance_method(:class)`
#     etc. later — Kernel now exists as a real Module (backed by
#     the VM's Class shell with `is_module: true`) with a methods
#     table where builtin Method records can be installed.
#
# Currently `Kernel` and `BasicObject` are empty stubs — their
# method tables don't carry the inline-handled primitives as
# Method records. However, `Kernel.instance_method(:class)` /
# `(:respond_to?)` etc. still work because `instance_method`
# treats Kernel as a primitive sentinel and synthesises an
# UnboundMethod whose dispatch routes through the receiver's
# normal method chain. What's missing is Method-record
# introspection: `m.arity`, `m.source_location`, `m.parameters`
# return defaults instead of the real values. Filling in real
# Method records on Kernel's methods table is tracked as a
# separate follow-up.

class BasicObject
end

module Kernel
end

# `Kernel#loop` — installed by ADR 0024 Phase A.3 (2026-05-30).
#
# Background (kept for historical context): pre-ADR-0024,
# Op::Yield was fire-and-forget and `def loop; while true;
# yield; end; end` hung infinitely on `loop { break }` because
# `break_signaled` was set but never observed by the yielding
# method's bytecode. ADR 0024 Phase A.1 (commit fd7fadc8) made
# Op::Yield synchronous + observe break_signaled, unblocking
# this canonical CRuby-faithful def.
#
# CRuby's `loop` also rescues StopIteration and returns the
# exception's `#result` attr. StopIteration was added in Phase
# A.2 (same session) so the rescue clause matches CRuby
# exactly. Embedders that don't go through external Enumerator
# iteration never trip the rescue; the path's there for
# parity.
#
# Top-level def (not inside `module Kernel`) because rubyrs's
# top-level dispatch walks `toplevel_methods`, not Kernel's
# method table — see `vm/dispatch.rs:7083` for that rationale.
def loop
  while true
    yield
  end
rescue StopIteration => e
  e.result
end

class Object < BasicObject
  include Kernel
  # Default reflection hook. `respond_to?` consults this only after
  # normal resolution misses; the base returns false so a user override
  # can `... || super` to fall back to it. PRIVATE, matching CRuby — so
  # `obj.respond_to?(:respond_to_missing?)` is false without the
  # include-private flag.
  def respond_to_missing?(name, include_private = false)
    false
  end
  private :respond_to_missing?
end

# Re-parent the exception hierarchy onto Object. preamble/exceptions.rb
# loads BEFORE this file (so RuntimeError/etc. resolve while the rest of
# the preamble loads), which means `class Exception` could not default
# its superclass to the not-yet-defined Object and was created as a ROOT
# (superclass = nil). Now that Object exists, reopen `Exception < Object`
# to match CRuby — `Exception.ancestors` becomes
# `[Exception, Object, Kernel, BasicObject]`. Without this, exception
# INSTANCES never resolve Ruby-level Object/Kernel methods (only the
# VM-special-cased natives like `frozen?`), so anything mixed into Object
# afterwards — e.g. minitest's `must_*` expectations on a rescued
# exception — is invisible. The reopen sets the superclass via the normal
# class-definition path (nil → Object only; it never clobbers an existing
# parent) AND runs the const-/method-generation invalidation that
# exception dispatch caching depends on. The whole subtree
# (StandardError < Exception, ...) shares the one Exception object, so a
# single reopen re-parents the lot.
class Exception < Object
end

## Phase C.1 Numeric / Rational class shells. CRuby's chain is
## `Rational < Numeric < Object`; the actual arithmetic is wired
## via primitive dispatch arms in the VM (numeric.rs / dispatch.rs),
## not via instance methods on these shells. Declaring them here
## ensures `Rational.new(...)` resolves (we shim `Kernel#Rational`
## as the public constructor entry) AND `obj.is_a?(Numeric)` works
## across Integer / Float / Rational.
##
## Re-opening `class Integer < Numeric` / `class Float < Numeric`
## here is intentional: Integer and Float already exist as
## seeded shells whose initial superclass is Object. The
## re-open form with an explicit superclass is rejected by
## CRuby ONLY when the new superclass differs from the
## existing one; declaring the superclass we WANT to apply on
## first definition (Object → Numeric) is the canonical way to
## promote them. The preamble runs once at boot, before any
## user code observes `Integer.superclass`, so the promotion
## is invisible to scripts that don't look.
## `Numeric` mixes in `Comparable` — but the `include` lives at
## the END of preamble/comparable.rb (which loads AFTER this
## fragment), because the `Comparable` constant doesn't exist
## yet at this point. See that file for the rationale.
class Numeric < Object
end
class Integer < Numeric
end
class Float < Numeric
end
class Rational < Numeric
end

## `Regexp` class shell — `/pattern/` literals are values of
## class Regexp; the constant needs to be reachable as a
## script-visible name so `x.is_a?(Regexp)` (sinatra-cors and
## a wider gem ecosystem use this) and `Regexp` as a typecase
## arm resolve. The instance surface (match, source, etc.)
## lives in the Rust-side `Value::Regex` arms; the class
## shell here is the *constant* needed for `is_a?` and
## `case/when Regexp` shapes.
class Regexp < Object
  ## Flag constants — CRuby's exact bitmask values. Consumed by
  ## `#options` (returns the OR of the set flags) and by gem code
  ## that tests `re.options & Regexp::EXTENDED`. The Ruby /m flag
  ## is "dot matches newline" (NOT multi-line `^`/`$`).
  IGNORECASE = 1
  EXTENDED   = 2
  MULTILINE  = 4
  ## Encoding-flag constants. rubyrs regexes have no per-regex
  ## encoding semantics (UTF-8 throughout; see the settled
  ## Regexp-over-non-UTF-8 boundary in SUBSET.md), so these bits
  ## are accepted-and-ignored by `Regexp.new` — they exist so
  ## option-passing callers load (rack's URLMap builds
  ## `Regexp.new(pattern, Regexp::NOENCODING)`).
  FIXEDENCODING = 16
  NOENCODING    = 32
end

# Marshal — binary serialization is out of the Tier-1 subset (no
# stable wire format commitment). The surface exists because real
# callers use `dump` as a DUMPABILITY PROBE, not for the bytes:
# minitest's exception sanitizer dumps every captured exception
# (and structurally requires the neutered-RuntimeError dump to
# SUCCEED — its "if this raises, we die" terminal). `dump`
# therefore returns a placeholder and never raises.
#
# The placeholder is VALID EMPTY YAML on purpose: Jekyll's
# regenerator writes `Marshal.dump(metadata)` to `.jekyll-metadata`
# and reads it back with `Marshal.load → rescue TypeError →
# SafeYAML.load`. Our `load` raises TypeError for ANY input (we
# can't parse real marshal bytes either — the same answer CRuby
# gives non-marshal input), so that fallback chain lands in
# SafeYAML, parses the placeholder to `{}`, and Jekyll degrades to
# a full rebuild — byte-identical output, no crash. A
# NotImplementedError here escaped regenerator's rescue list and
# aborted real builds (caught by the jk-real byte-identity gate).
module Marshal
  # Same-process round-trip: dump stashes the object in the VM
  # registry and returns a token (still valid YAML — an empty hash
  # plus a comment — so disk consumers degrade through SafeYAML
  # fallbacks); load of that exact token returns the SAME object.
  # DIVERGENCES (documented): shallow (CRuby deep-copies through
  # the byte stream — mutations are shared here); tokens are
  # process-local (a dump written to disk and loaded by another
  # run raises TypeError, the honest answer that rescue chains
  # like Jekyll's regenerator already handle); registry caps at
  # 1024 dumps, after which dump degrades to the tokenless
  # placeholder. minitest's Result over-the-wire tests only need
  # the same-process equality contract.
  def self.dump(obj, *_rest)
    # Prefer a REAL CRuby-4.8 byte stream for the common-tag subset
    # (nil/bool/Integer/Float/String/Symbol/Array/Hash + links). That
    # makes load(dump(x)) a genuine DEEP COPY and the bytes portable to
    # CRuby. Anything outside the subset (Bignum, arbitrary objects,
    # Struct, Procs, Hash-with-default, …) returns nil here and falls
    # back to the same-process registry token — which still satisfies
    # load(dump(x)) == x and preserves object identity for the types
    # that can't be byte-serialized (minitest's Result contract).
    bin = __rubyrs_marshal_dump_binary(obj)
    return bin unless bin.nil?
    __rubyrs_marshal_stash(obj)
  end

  def self.load(src, *_rest)
    s = src.to_s
    hit = __rubyrs_marshal_fetch(s)
    return hit[0] if hit
    # Real CRuby marshal bytes (\x04\x08 header): the load-only
    # binary reader handles the common-tag subset (nil/bool/int/
    # float/string/symbol/array/hash + links); anything richer
    # raises TypeError naming the tag. Consumer: addressable's
    # pregenerated unicode.data table.
    if s.getbyte(0) == 4 && s.getbyte(1) == 8
      return __rubyrs_marshal_load_binary(s)
    end
    raise TypeError,
      "incompatible marshal file format (rubyrs Tier 1: token round-trip or common-tag binary subset)"
  end
end

# Binding — captures the calling scope for later `eval`. The class is
# defined here so the instance exists (and Marshal still REJECTs it —
# no _dump_data — routing such exceptions into minitest's neuter
# chain). The `Kernel#binding` factory itself is a NATIVE builtin
# (vm/kernel.rs) so it can capture the live frame's self + lexical
# class into @__self / @__lexical_class — `eval(src, binding)` reads
# those to run with the captured self (rack's Builder.new_from_string
# evals a rackup script against `builder.instance_eval { binding }`).
# Outer local-variable capture is a follow-up.
class Binding
end

# ObjectSpace::WeakMap — a map whose entries don't keep their keys/
# values alive. Tier-1 models the MAP API only; the WEAK part is a
# documented DIVERGENCE: entries are held with STRONG references and
# never get collected (rubyrs's GC has no weak-ref table). Consumers
# that use it as a leak-tolerant cache or registry still work —
# connection_pool tracks live pools in `INSTANCES = WeakMap.new` for
# its after-fork cleanup; ActiveSupport uses it for descendant
# tracking. Code that DEPENDS on entries vanishing after GC will see
# them linger (the cost of the single-process Tier-1 model).
#
# Keys compare by IDENTITY, not eql?/hash (CRuby: `w["x"]` misses an
# entry stored under a different but equal `"x"`), so the backing Hash
# is keyed on `object_id` and stores `[key, value]` pairs. The
# `ObjectSpace` module itself is otherwise unmodelled (no each_object).
module ObjectSpace
  # `define_finalizer(obj, callable=nil)` / `undefine_finalizer(obj)` —
  # CRuby registers a proc to run when `obj` is garbage-collected. rubyrs
  # has no GC-finalizer hook (drop-based, single-process Tier-1), so these
  # are no-ops: the registration is accepted but the finalizer never
  # fires. Matches CRuby's "finalizers are best-effort / unordered /
  # may-not-run" contract for the common cleanup-cache use (mustermann's
  # EqualityMap registers one to evict cached patterns — here the cache
  # simply never evicts). Return shape mirrors CRuby:
  # `[0, callable]`-ish — we return the object for chaining tolerance.
  def self.define_finalizer(obj, callable = nil, &block)
    # CRuby returns `[0, callable]` (the "table slot" 0 + the finalizer);
    # the block form uses the block as the callable.
    [0, callable || block]
  end

  def self.undefine_finalizer(obj)
    obj
  end

  # NOTE: CRuby's WeakMap includes Enumerable, but object.rb loads
  # before enumerable.rb in the preamble, so we don't mix it in here
  # (the named `each_*` / `keys` / `values` cover the consumed surface;
  # add Enumerable later if a gem needs `map`/`select`/etc. on a WeakMap).
  class WeakMap
    def initialize
      @entries = {}   # object_id => [key, value]
    end

    def [](key)
      e = @entries[key.object_id]
      e && e[1]
    end

    def []=(key, value)
      @entries[key.object_id] = [key, value]
      value
    end

    def key?(key)
      @entries.key?(key.object_id)
    end
    alias_method :include?, :key?
    alias_method :member?, :key?

    def delete(key)
      e = @entries.delete(key.object_id)
      e && e[1]
    end

    def each
      return enum_for(:each) unless block_given?
      @entries.each_value { |(k, v)| yield k, v }
      self
    end
    alias_method :each_pair, :each

    def each_key
      return enum_for(:each_key) unless block_given?
      @entries.each_value { |(k, _v)| yield k }
      self
    end

    def each_value
      return enum_for(:each_value) unless block_given?
      @entries.each_value { |(_k, v)| yield v }
      self
    end

    def keys
      @entries.values.map { |(k, _v)| k }
    end

    def values
      @entries.values.map { |(_k, v)| v }
    end

    def size
      @entries.size
    end
    alias_method :length, :size

    def inspect
      "#<ObjectSpace::WeakMap:0x#{object_id.to_s(16)} size=#{@entries.size}>"
    end
  end
end

# GC — rubyrs has no user-triggerable collector (Tier-1 relies on the
# host's allocator / drop semantics), so every entry point is an honest
# no-op that returns CRuby's value: `start` → nil, `enable`/`disable` →
# false (the previous "was disabled" state, which is always false here),
# `count` → 0 (no collections have run). Real gems call `GC.start` in
# benchmarks/teardown and `GC.disable` around hot loops; defining the
# module lets that code run instead of tripping a NameError. `GC.stat`
# is deliberately omitted — fabricating a stats Hash would mislead code
# that reads its keys; an absent method is the truthful surface.
module GC
  def self.start(*); nil; end
  def self.enable; false; end
  def self.disable; false; end
  def self.count; 0; end
end
