# The core metaclass hierarchy: Module < Object, Class < Module. A module
# or class VALUE is therefore an Object, so `Object === mod` / `mod.is_a?
# (Object)` / `case mod when Object` hold (dry-core's class-attribute
# `type === value` check with `type: Object` relies on this).
module M; end
class C; end
p Module.superclass            # Object
p Class.superclass             # Module
p Module.ancestors             # [Module, Object, Kernel, BasicObject]
p Class.ancestors              # [Class, Module, Object, Kernel, BasicObject]
p(Object === M)                # true
p(Object === C)                # true
p(Object === Object)           # true
p M.is_a?(Object)              # true
p C.is_a?(Object)              # true
p M.is_a?(Module)              # true
p C.is_a?(Class)               # true
p C.is_a?(Module)              # true
p Module.is_a?(Class)          # true
p Comparable.is_a?(Module)     # true
p Comparable.is_a?(Class)      # false
case M
when Class then p :class
when Module then p :module
end
case C
when Class then p :class
when Module then p :module
end
p [Integer, String, Comparable].all? { |k| Object === k }  # true
