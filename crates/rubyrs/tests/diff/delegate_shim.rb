# `delegate` stdlib shim — `Delegator`, `SimpleDelegator`,
# and the top-level `DelegateClass(...)` factory. Pre-shim,
# `class X < DelegateClass(Y)` tripped `NoMethodError:
# undefined method 'DelegateClass' for Class` at class-body
# evaluation.
#
# P3 Sinatra spike — Mustermann's
# `mustermann/ast/translator.rb:18` reads
#   class NodeTranslator < DelegateClass(Node)
# inside a module body; the require chain through Sinatra
# pulls this in at load time.

require "delegate"

class Inner
  attr_reader :tag
  def initialize(tag); @tag = tag; end
  def shout; "INNER<#{@tag}>"; end
  def square(n); n * n; end
end

# 1. SimpleDelegator forwards every undefined method to the
# wrapped object via method_missing.
sd = SimpleDelegator.new(Inner.new("a"))
puts "sd_tag=#{sd.tag}"
puts "sd_shout=#{sd.shout}"
puts "sd_square=#{sd.square(7)}"

# 2. SimpleDelegator is_a? Delegator (the class hierarchy
# the kernel-stub shells anchor must hold post-shim).
puts "sd_is_delegator=#{sd.is_a?(Delegator)}"
puts "sd_class=#{sd.class}"

# 3. `__getobj__` / `__setobj__` round-trip — swappable
# inner.
sd2 = SimpleDelegator.new(Inner.new("first"))
puts "sd2_pre=#{sd2.tag}"
sd2.__setobj__(Inner.new("second"))
puts "sd2_post=#{sd2.tag}"
got = sd2.__getobj__
puts "got_tag=#{got.tag}"

# 4. Top-level `DelegateClass(SomeClass)` returns a Class
# valid as a `<` superclass. Subclass methods coexist with
# the delegated surface.
class Wrapped < DelegateClass(Inner)
  def my_helper; "wrap_only"; end
end
w = Wrapped.new(Inner.new("dc"))
puts "w_tag=#{w.tag}"             # delegated
puts "w_shout=#{w.shout}"         # delegated
puts "w_helper=#{w.my_helper}"    # own
puts "w_is_delegator=#{w.is_a?(Delegator)}"

# 5. Class equality — `Wrapped < DelegateClass(Inner)`
# pseudo-inherits Delegator (rubyrs's shim returns Delegator
# from the factory rather than `Class.new(Delegator)`,
# documented divergence in delegate_shim.rb). Verify a
# subclass chain still holds via `is_a?` rather than
# `respond_to?` — the latter doesn't yet consult
# `respond_to_missing?` (rubyrs gap), so a real feature-
# detection scenario would diverge from CRuby.
puts "w_kind_of_inner=#{Wrapped.new(Inner.new("k")).kind_of?(Delegator)}"
