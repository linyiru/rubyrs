# `UnboundMethod#bind_call(recv, *args)` — CRuby 2.7+ fused
# `bind(recv).call(*args)` without the transient BoundMethod
# allocation.
#
# Motivating consumer: tilt-2.7.0 `lib/tilt/template.rb:496` calls
# `method.bind_call(scope, **locals, &block)` per render — and
# `compile_template_method` captures via
# `TOPOBJECT.instance_method(name)` then `remove_method(name)`
# BEFORE the call, so the UnboundMethod has to survive a
# capture-then-removal of its source slot.
#
# Coverage:
#   - Class capture: bind_call works on instance of captured class
#   - Class capture: is_a? mismatch raises TypeError
#   - Module capture: bind_call works on ANY object (CRuby parity:
#     module instance_methods aren't is_a?-fenced)
#   - Capture-then-remove-then-call: the snapshotted Method
#     survives a subsequent `remove_method` (tilt's pattern)
#   - Args + return value forwarded correctly
#   - 0-arg shape raises ArgumentError

# --- Class capture: works on matching instance ---
class Greeter
  def greet(name)
    "hello, #{name}"
  end
end
m = Greeter.instance_method(:greet)
puts m.bind_call(Greeter.new, "world")            # hello, world

# --- Class capture: wrong receiver class raises TypeError ---
class Other
end
begin
  m.bind_call(Other.new, "x")
rescue TypeError
  puts "wrong-class → TypeError"
end

# --- Module capture: any object accepted (CRuby parity) ---
module Greeter2
  def greet
    "hello from M"
  end
end
mm = Greeter2.instance_method(:greet)
puts mm.bind_call(Object.new)                      # hello from M

# --- Capture-then-remove-then-bind_call: snapshot survives ---
module CompiledLike
  def __tilt_42
    "compiled body"
  end
end
captured = CompiledLike.instance_method(:__tilt_42)
CompiledLike.class_eval { remove_method(:__tilt_42) }
puts captured.bind_call(Object.new)                # compiled body

# --- 0-arg shape: ArgumentError ---
begin
  m.bind_call
rescue ArgumentError
  puts "0-arg → ArgumentError"
end

# --- respond_to? whitelist consistency ---
puts m.respond_to?(:bind_call)                     # true
