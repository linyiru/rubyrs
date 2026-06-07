# Core Enumerable is a Module (not a Class), so `Mod.include?(Enumerable)`
# passes the expected-Module check and `class X; include Enumerable`.
p Enumerable.is_a?(Module)
p Enumerable.instance_of?(Class)
class Box; include Enumerable; def each; yield 1; yield 2; end; end
p Box.include?(Enumerable)
p Box.new.respond_to?(:each)
