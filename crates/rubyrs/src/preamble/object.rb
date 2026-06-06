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
