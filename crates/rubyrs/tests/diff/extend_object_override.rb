# `extend M` dispatches an overridden `Module#extend_object` (its `super`
# does the real singleton insert), not just the `extended` hook.
module Ext
  def self.extend_object(obj)
    super
    obj.instance_variable_set(:@marked, true)
  end
  def helper; "helped"; end
end
m = Module.new { extend Ext }
p m.helper
p m.instance_variable_get(:@marked)
