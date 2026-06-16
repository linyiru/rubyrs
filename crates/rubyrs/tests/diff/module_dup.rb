# `Module#dup` / `Class#dup` shallow-copies the method / singleton-method
# tables into a fresh ANONYMOUS module (name follows a later constant
# assignment). Surfaced by the `inclusive` gem's `ModuleWithPackages.dup`
# (bridgetown-foundation's packages DSL).
module M
  def self.foo = 42
  def self.[](x) = x * 2
  def hello = "hi"
end
d = M.dup
puts d.class
puts d.name.inspect
puts d.foo
puts d[5]
k = Class.new { include d }
puts k.new.hello
