# `Module.instance_method(:name).bind_call(mod)` — reflective capture of
# the native Module#name, bypassing any override. zeitwerk's RealModName
# uses exactly this to read a module's real name.
module Alpha; end
class Beta; end
module Outer
  module Inner; end
end

UM = Module.instance_method(:name)
p UM.bind_call(Alpha)
p UM.bind_call(Beta)
p UM.bind_call(Outer::Inner)

# Anonymous module → nil name.
p UM.bind_call(Module.new)

# Class.instance_method(:name) resolves the same native method (Class < Module).
p Class.instance_method(:name).bind_call(Beta)
