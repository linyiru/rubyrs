# `respond_to?` consults a user-defined `respond_to_missing?` when normal
# resolution misses — the reflection companion to `method_missing` for
# proxy / DSL objects. The base `Object#respond_to_missing?` returns false
# so an override can `... || super`.

class Proxy
  def respond_to_missing?(name, include_private = false)
    name.to_s.start_with?("dynamic_") || super
  end

  def method_missing(name, *args)
    name.to_s.start_with?("dynamic_") ? "handled #{name}" : super
  end
end

pr = Proxy.new
p pr.respond_to?(:dynamic_foo)         # true (via respond_to_missing?)
p pr.respond_to?(:other)               # false (super → Object default)
p pr.respond_to?(:dynamic_bar, true)   # true, include_private passed through
p pr.dynamic_baz                        # "handled dynamic_baz"
p pr.respond_to?(:class)               # true (normal resolution)

# no override → base Object#respond_to_missing? → false
p Object.new.respond_to?(:nope)
p 5.respond_to?(:nope)
p "x".respond_to?(:nope)
p [].respond_to?(:totally_absent)

# the hook is itself a real method
p Object.new.respond_to?(:respond_to_missing?)

# include_private flag reaches the override
class Flagged
  def respond_to_missing?(name, include_private = false)
    include_private && name == :secret
  end
end
f = Flagged.new
p f.respond_to?(:secret)         # false (include_private defaults false)
p f.respond_to?(:secret, true)   # true
