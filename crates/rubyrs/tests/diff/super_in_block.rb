# `super` inside an iter block forwards to the enclosing METHOD's
# super-chain. CRuby walks past intermediate block frames; the
# block's `defining_class` is None, so super_lookup must walk the
# frame stack to find the enclosing method frame.
# Surfaced by sinatra-contrib/MultiRoute's verb iteration:
#   args.each do |verb|
#     routes.each do |route|
#       super(verb, route, options, &block)
#     end
#   end

class Base
  def self.greet(x); "Base[#{x}]"; end
end

module Wrapper
  def greet(*args)
    args.each do |a|
      puts super(a)
    end
  end
end

class App < Base
  extend Wrapper
end

App.greet(1, 2, 3)

# Nested blocks — super still resolves through both.
class App2 < Base
  extend(Module.new do
    def greet(*args)
      args.each do |outer|
        [outer * 2].each do |inner|
          puts super(inner)
        end
      end
    end
  end)
end

App2.greet(10, 20)

# Instance-method version — `super(arg)` inside a block resolves
# through the regular instance-method super-chain.
class InstanceBase
  def go(x); "InstanceBase[#{x}]"; end
end
class Sub < InstanceBase
  def go(args)
    args.each { |a| puts super(a) }
  end
end
Sub.new.go([10, 20])
