# `super` from an overridden `include`/`extend`/`prepend` reaching the
# builtin `Module#include` etc. Mirrors concurrent-ruby's
# `Concurrent::ReInclude` (extended onto a module, overrides `include`
# and calls `super(*modules)`).
module Reinc
  def include(*mods)
    @inc_log = (@inc_log || []) + mods.map(&:name)
    super(*mods)
  end
  def extend_obj(mod)
    super
  end
end

module Extra
  def hello; "hi-from-extra"; end
end

module Greetable
  def greet; "greet!"; end
end

module Host
  extend Reinc
  include Extra
end

class Consumer
  include Host
end

puts Consumer.new.hello
puts Host.instance_variable_get(:@inc_log).inspect
puts Host.ancestors.include?(Extra)

# extend super: a module overriding extend and calling super
module ExtReinc
  def extend(*mods)
    @ext_log = (@ext_log || []) + mods.map(&:name)
    super(*mods)
  end
end

obj = Object.new
obj.singleton_class.send(:define_method, :base) { "base" }
# Use a class that overrides extend at the singleton level
module Target
  extend ExtReinc
  extend Greetable
end
puts Target.greet
puts Target.instance_variable_get(:@ext_log).inspect
