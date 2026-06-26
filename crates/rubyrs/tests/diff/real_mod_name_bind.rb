# `Module.instance_method(:name).bind_call(mod)` / `.bind(mod).call` must
# return mod's REAL constant name, bypassing a `def self.name` override —
# CRuby invokes the captured builtin, not the override. This is zeitwerk's
# RealModName, used pervasively for cpath computation under custom inflectors.
module M
  def self.name; "Overridden"; end
end
um = Module.instance_method(:name)
puts um.bind_call(M)        # "M"
puts um.bind(M).call        # "M"

# Nested + a normal (non-overridden) module still report their real names.
module Outer
  module Inner; end
end
puts um.bind_call(Outer::Inner)   # "Outer::Inner"

# Anonymous module → nil (no name), printed as a blank line.
anon = Module.new
p um.bind_call(anon)        # nil
