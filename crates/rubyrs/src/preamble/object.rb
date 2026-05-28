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

class Object < BasicObject
  include Kernel
end
