# `method_missing` defined in a module extended into a Class /
# Module receiver is reached on unknown class-method calls.
# Pre-fix `try_method_missing` only triggered for Value::Object
# receivers, swallowing recorders like sinatra-contrib/Extension
# which lives in a Module extended with a method_missing-bearing
# helper module.

# Plain instance-side method_missing — baseline.
class K
  def method_missing(name, *args)
    "K_MM:#{name}:#{args.inspect}"
  end
end
puts K.new.something(1, 2)

# Module extended into another Module — Target.something walks
# the singleton-include chain and hits M's method_missing.
module M
  def method_missing(name, *args)
    "M_MM:#{name}:#{args.inspect}"
  end
end

module Target
  extend M
end
puts Target.something("a", "b")

# Same shape but the receiver is a Class. Klass.foo when foo
# isn't defined consults the class's singleton chain (including
# extended modules), then method_missing.
class Klass
  extend M
end
puts Klass.unknown_class_method(:x)

# Existing class methods still dispatch normally — method_missing
# only fires on the MISS path.
class K2
  extend M
  def self.real; "real_class_method"; end
end
puts K2.real           # not MM
puts K2.absent(:arg)   # MM

# Block-form call still reaches method_missing.
class K3
  extend M
end
result = K3.with_block(:tag) { "from_block" }
puts result

# super still works inside method_missing — call super to fall
# through to NoMethodError if the user wants to filter.
module MFilter
  def method_missing(name, *args, &block)
    return "MFilter_handled:#{name}" if name.to_s.start_with?("greet_")
    super
  end
end

class K4
  extend MFilter
end
puts K4.greet_world
puts(begin K4.totally_unknown; rescue NoMethodError => e; "raised: #{e.class}"; end)
