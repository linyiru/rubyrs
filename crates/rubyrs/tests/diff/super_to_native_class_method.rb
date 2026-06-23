# A user override of a NATIVE class/module method (autoload, const_set,
# define_method, ...) that calls `super` must reach the native impl —
# not recurse into itself. ActiveSupport::Dependencies::Autoload#autoload
# does `super const, path`. The super-fallback force-primitive-dispatches
# the native method, with the class-singleton dispatch gated so it won't
# re-find the override.
module Mixin
  def autoload(const, path = nil)
    super(const, "vendor/#{path}")
  end
end
module Host
  extend Mixin
  autoload :Thing, "thing"
end
p Host.autoload?(:Thing)              # "vendor/thing"

# super to const_set from an override.
module CSetMixin
  def const_set(name, val)
    super(name, val * 2)
  end
end
module CHost
  extend CSetMixin
  const_set(:N, 21)
end
p CHost::N                            # 42
