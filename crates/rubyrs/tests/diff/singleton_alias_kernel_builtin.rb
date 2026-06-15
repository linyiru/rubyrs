# Aliasing a (private) Kernel builtin as a SINGLETON method inside a
# `class << self` body — zeitwerk's core_ext/kernel.rb does
# `module Kernel; class << self; alias_method
# :zeitwerk_original_require, :require; end; end`. The alias inherits
# the builtin's PRIVATE visibility, so an explicit-receiver call
# raises while implicit-self dispatch resolves.
module Foo
  class << self
    alias_method :my_print, :print
    alias_method :my_p, :p
  end

  # Implicit-self call from a public wrapper reaches the private alias.
  def self.shout(s)
    my_print("shout:#{s}\n")
  end
end

Foo.shout("hi")
p Foo.respond_to?(:my_print)         # false — private
p Foo.respond_to?(:my_print, true)   # true  — include private
begin
  Foo.my_print("nope\n")
rescue NoMethodError => e
  p :private_blocked
end
