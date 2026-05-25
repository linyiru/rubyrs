# Kernel.instance_method(:foo).bind(obj).call — the canonical
# CRuby idiom for invoking a method on an arbitrary receiver
# without going through `send` (tilt/template.rb:238 uses this
# at boot to grab `Object#class` for later use). Also covers
# RUBY_VERSION / RUBY_PLATFORM preamble constants which tilt
# uses on the very next line for version-conditional dispatch.

# `Kernel.instance_method(:class)` succeeds — returns an
# UnboundMethod over the universally-available `class` method.
m = Kernel.instance_method(:class)
puts m.class

# Bind to a primitive — works for any value (CRuby: Kernel is
# included in Object, so every value is_a Kernel).
puts m.bind("hi").call
puts m.bind(42).call
puts m.bind([]).call
puts m.bind({}).call
puts m.bind(:sym).call
puts m.bind(true).call
puts m.bind(nil).call

# Bind to a user instance — calls into the receiver's class
# dispatch, returning the actual class.
class Greeter
  def hello; "hi"; end
end
puts m.bind(Greeter.new).call

# RUBY_VERSION / RUBY_PLATFORM are pre-defined preamble strings.
# We claim a recent CRuby version so version-conditional code
# opts into the modern branch — most real codebases gate behind
# `>= '2.7'` or `>= '3'`.
puts RUBY_VERSION.is_a?(String)
puts RUBY_VERSION >= '2.7'
puts RUBY_VERSION >= '3'
puts RUBY_PLATFORM.is_a?(String)

# Constant assignment with Kernel UnboundMethod — the exact tilt
# pattern (`CLASS_METHOD = Kernel.instance_method(:class)` at
# top-level), stored once and called many times.
CLASS_METHOD = Kernel.instance_method(:class)
USE_BIND_CALL = RUBY_VERSION >= '3'
puts USE_BIND_CALL
puts CLASS_METHOD.bind("via constant").call
