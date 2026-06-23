# User-defined Module subclasses: `class Tagged < Module` — an instance
# of a Module subclass IS a module (extend-able, own method table, fires
# the `extended` hook, reports the subclass as its class). dry-core's
# Deprecations::Tagged / Equalizer / ClassAttributes build the whole
# dry-rb stack on this.
module Interface
  def deprecation_tag(tag = nil)
    defined?(@deprecation_tag) ? @deprecation_tag : (@deprecation_tag = tag)
  end
end

class Tagged < ::Module
  def initialize(tag)
    super()
    @tag = tag
  end
  def extended(base)         # fires on `extend Tagged.new(...)`
    base.extend Interface
    base.deprecation_tag @tag
  end
  def label                   # instance method, called on the module value
    "tag=#{@tag}"
  end
end

m = Tagged.new("lib-x")
p m.class                     # Tagged
p m.is_a?(Module)             # true
p m.is_a?(Object)             # true
p m.instance_of?(Tagged)     # true
p m.label                     # "tag=lib-x"  (instance-method dispatch on module value)
p m.instance_variable_get(:@tag)  # "lib-x"

class Thing
  extend Tagged.new("my-lib")
end
p Thing.deprecation_tag       # "my-lib"  (extended hook ran)

# Module.new with a block still works (block = module body).
mod = Module.new do
  def greet; "hi"; end
end
c = Class.new { include mod }
p c.new.greet                 # "hi"
