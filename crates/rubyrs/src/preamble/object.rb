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

# Subset gap: `Kernel#loop` is intentionally NOT installed.
# Investigated on 2026-05-29 (discovered while writing the
# client-disconnect close test for ADR 0023 Risk #1) and
# deferred.
#
# Three implementation paths exist; none are quick wins:
#
# 1. Ruby-level `def loop; while true; yield; end; end` —
#    works for `next` and natural fallthrough, but `break`
#    inside the block sets `vm.break_signaled` without
#    propagating: `Op::Yield` doesn't check the flag, so
#    after the block returns the `while true` loops again
#    and yields again. `loop { break }` hangs in an infinite
#    Ruby loop. A clean NoMethodError beats a silent hang —
#    so this option is rejected.
#
# 2. Rust builtin in `vm/kernel.rs` (mirrors Int#times) —
#    correctly handles `break` via `BlockStep::Break` but
#    inherits the silent-truncation guard added in
#    `vm::iter::step_block` for Fiber-driven streaming
#    (P2 #21 follow-up). `loop { yield_to_fiber }` would
#    deliver one chunk and silently drop the rest — same
#    UX as the documented `times`-inside-Fiber limitation.
#    Also non-trivial: builtin_call has no block parameter,
#    so the impl needs to route through the do_call_block
#    dispatch path's no-recv arm, not via `builtin_call`.
#
# 3. Proper block-break propagation in `Op::Yield` — the
#    "right" fix. Block-break in CRuby unwinds the
#    YIELDING method (not lexically-defining method like
#    `return` does). rubyrs has `method_return` for return-
#    from-block; an analogous `block_break_return` flag
#    consumed at `Op::Yield`'s return path would unblock
#    user-level `def`s that wrap iteration. Significant
#    bytecode semantics work, deserves its own ADR.
#
# Until one of those lands, scripts use `while true` instead
# of `loop`. The SSE example (`crates/rubyrs/examples/sse_server.rb`)
# and the streaming tests already use this idiom.

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
class Numeric < Object
end
class Rational < Numeric
end
