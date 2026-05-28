# Kernel built-in Method reflection — Step 2 follow-up to the
# BasicObject/Kernel/Object root hierarchy (PR #256).
#
# `Kernel.instance_method(:foo)` previously returned an
# UnboundMethod with no Method snapshot (the primitive-sentinel
# path), so reflection (`arity`, `parameters`, `source_location`)
# returned defaults: arity=-1, parameters=[[:rest]], source=nil.
#
# This PR adds a synthesised Method registry on the Vm
# (`kernel_builtin_metas`) consulted by the `instance_method`
# arm when the receiver is Kernel. The synth records carry
# real arity/parameters/source_label metadata. They are NOT
# inserted onto Kernel.methods — keeping them off the chain
# avoids re-finding the synth during regular dispatch
# (`obj.class` etc.) which would cause infinite recursion or
# spurious user-override signals.
#
# Invocation via `bind_call` routes through `invoke_method`'s
# builtin short-circuit back into `do_call(primitive_name)`,
# where the inline primitive arm (`obj.class`'s `class_of`,
# etc.) handles the actual work.

# --- Zero-arg accessor: arity 0, params [] ---
m = Kernel.instance_method(:class)
puts m.arity                                          # 0
puts m.parameters.inspect                             # []
puts m.owner                                          # Kernel
puts m.source_location.first                          # "<internal:kernel>"

# --- Single-arg predicate: arity 1, params [[:req]] ---
ia = Kernel.instance_method(:is_a?)
puts ia.arity                                         # 1
puts ia.parameters.inspect                            # [[:req]]
puts ia.owner                                         # Kernel

# --- Variadic: arity -1, params [[:rest]] ---
rt = Kernel.instance_method(:respond_to?)
puts rt.arity                                         # -1
puts rt.parameters.inspect                            # [[:rest]]

snd = Kernel.instance_method(:send)
puts snd.arity                                        # -1
puts snd.parameters.inspect                           # [[:rest]]

# --- bind_call routes through inline primitive dispatch ---
m = Kernel.instance_method(:class)
puts m.bind_call("hello")                             # String
puts m.bind_call(42)                                  # Integer
puts m.bind_call([1, 2])                              # Array
class V
end
puts m.bind_call(V.new)                               # V

ia = Kernel.instance_method(:is_a?)
puts ia.bind_call(42, Integer)                        # true
puts ia.bind_call("x", Integer)                       # false

# --- Aliased dispatch still works (regression guard for the
#     "synth on chain" pitfall — the synth lives in the
#     registry, NOT on Kernel.methods, so aliasing via
#     `class Symbol; alias_method :show, :to_s; end` finds
#     Symbol's primitive to_s as before, not the synth) ---
class Symbol
  alias_method :show, :to_s
end
puts :bar.show                                        # bar
[:a, :b, :c].each { |s| puts s.show }                 # a / b / c
