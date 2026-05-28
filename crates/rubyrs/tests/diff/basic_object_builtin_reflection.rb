# BasicObject built-in Method reflection — Step 3 follow-up to
# the Kernel reflection PR (Step 2). CRuby defines a small set
# of methods directly on BasicObject (the root), distinct from
# Kernel's set:
#
#   BasicObject: __id__, __send__, equal?, instance_eval,
#                instance_exec, !, ==, !=
#
# Same off-chain registry design as Kernel — `Vm.basic_object_
# builtin_metas` stores the reflection metadata; the synth
# Methods are NOT inserted onto BasicObject.methods, so chain-
# walking dispatch doesn't re-find them.
#
# This PR also corrects a CRuby-divergence in Step 2: `equal?`
# and `__send__` were initially installed on Kernel; CRuby puts
# them on BasicObject only. Removed from Kernel.

# --- BasicObject reflection: arity 0 ---
m = BasicObject.instance_method(:__id__)
puts m.arity                                       # 0
puts m.parameters.inspect                          # []
puts m.owner                                       # BasicObject
puts m.source_location.inspect                     # nil (CRuby parity)

n = BasicObject.instance_method(:!)
puts n.arity                                       # 0
puts n.parameters.inspect                          # []

# --- BasicObject reflection: arity 1 ---
eq = BasicObject.instance_method(:equal?)
puts eq.arity                                      # 1
puts eq.parameters.inspect                         # [[:req]]

eqeq = BasicObject.instance_method(:==)
puts eqeq.arity                                    # 1

ne = BasicObject.instance_method(:!=)
puts ne.arity                                      # 1

# --- BasicObject reflection: arity -1 (variadic) ---
ie = BasicObject.instance_method(:instance_exec)
puts ie.arity                                      # -1
puts ie.parameters.inspect                         # [[:rest]]

iev = BasicObject.instance_method(:instance_eval)
puts iev.arity                                     # -1

snd = BasicObject.instance_method(:__send__)
puts snd.arity                                     # -1

# --- Inherited reflection: User → Object → includes Kernel ---
# Predates this PR (registry was originally direct-receiver-only)
# but adopted from the cycle-6 code-review finding. A user class
# that inherits Kernel via the standard `class User; end` (post
# PR #256's default-superclass-to-Object) should surface the
# Kernel synth via `User.instance_method(:class)` — the
# reflection should look identical to calling on Kernel directly.
class User; end
m_inh = User.instance_method(:class)
puts m_inh.arity                                  # 0
puts m_inh.owner                                  # Kernel
puts m_inh.parameters.inspect                     # []

# Multi-level: Sub → User → Object → Kernel chain
class Sub < User; end
m_sub = Sub.instance_method(:class)
puts m_sub.arity                                  # 0
puts m_sub.owner                                  # Kernel

# BasicObject-only opt-out: skips Kernel entirely, but BO
# reflection still works
class MinReceiver < BasicObject; end
m_min = MinReceiver.instance_method(:__id__)
puts m_min.arity                                  # 0
puts m_min.owner                                  # BasicObject

# Inherited reflection does not invent methods — non-registered
# names still raise NameError on user classes.
begin
  User.instance_method(:totally_made_up_method_xyz)
  puts "no raise (BAD)"
rescue NameError
  puts "NameError on unregistered name"
end

# --- bind_call: equal? routes through the universal arm ---
# `equal?` has a universal inline dispatch arm so bind_call
# works end-to-end. Other BasicObject methods (e.g. `__id__`)
# have reflection metadata but not inline dispatch — the
# reflection-only surface is the Step-3 ship; full invocation
# wiring is tracked separately.
m_eq = BasicObject.instance_method(:equal?)
x = Object.new
puts m_eq.bind_call(x, x)                          # true
puts m_eq.bind_call(x, Object.new)                 # false
