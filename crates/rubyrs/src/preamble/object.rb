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
    __rubyrs_marshal_stash(obj)
  end

  def self.load(src, *_rest)
    hit = __rubyrs_marshal_fetch(src.to_s)
    unless hit
      raise TypeError,
        "incompatible marshal file format (rubyrs Tier 1: same-process token round-trip only)"
    end
    hit[0]
  end
end

# Binding — Tier-1 opaque context token. CRuby's Binding captures
# the full lexical environment for later eval; rubyrs has no
# eval-in-binding, so `binding` returns an inert marker instance.
# It exists because real code stores one in an ivar as a "context
# I might inspect later" breadcrumb (minitest's BetterError test
# fixture does `@bad_ivar = binding` inside set_backtrace) — and
# because Marshal must REJECT it (no _dump_data), which is what
# routes such exceptions into minitest's neuter chain. Calling
# eval/local_variables on it raises NoMethodError (honest absence).
class Binding
end

def binding
  Binding.new
end
