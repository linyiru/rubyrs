# Universal ancestor hierarchy: BasicObject → Kernel (module,
# mixed into Object) → Object. Mirrors CRuby's actual chain
# instead of an isolated Object stub.
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
#     etc. later — the Kernel module now exists as a real Class
#     with a methods table where builtin Method records can be
#     installed.
#
# Currently `Kernel` and `BasicObject` are empty stubs — their
# method tables don't carry the inline-handled primitives yet.
# `Object.new.is_a?(Kernel)` returns true because the include
# is in the chain, and `Kernel.instance_method(:respond_to?)`
# still fails because the builtin isn't materialised. Synthesis
# of the inline-primitive Method records is tracked as a
# separate follow-up.

class BasicObject
end

module Kernel
end

class Object < BasicObject
  include Kernel
end
