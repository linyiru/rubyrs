# `super(*args, &block)` — splat AND explicit block forwarded
# together. Surfaced by sinatra-contrib/MultiRoute's verb
# override shape: `def get(*args, &block); super(*route_args(args),
# &block); end` — both channels must reach the inherited method.

# Baseline: bare super without splat already forwards block
# implicitly inside a regular call. The interesting case is
# `super(*args, &block)` with both an args-rebuild and an
# explicit block-arg.

module Wrap
  def call(*args, &block)
    "Wrap[#{args.inspect}]->" + super(*args.map(&:upcase), &block)
  end
end
class Base
  def self.call(*args, &block)
    "Base[#{args.inspect}, block=#{block&.call.inspect}]"
  end
end
class App < Base
  extend Wrap
end

puts App.call("hi", "lo") { "BLK" }

# Instance-method form — same shape, but the super walk goes
# through the regular instance ancestor chain.
class Greeter
  def greet(*args, &block)
    "Greeter[#{args.inspect}, block=#{block&.call.inspect}]"
  end
end
class LoudGreeter < Greeter
  def greet(*args, &block)
    "Loud->" + super(*args.map(&:upcase), &block)
  end
end
puts LoudGreeter.new.greet("hi", "world") { "yay" }

# Forwarding nil block (`&nil` shape — args present, no block).
class Quiet < Greeter
  def greet(*args)
    super(*args, &nil)
  end
end
puts Quiet.new.greet("hello")

# Block-only super (no args splat, just `&blk`). This still
# routes through the Apply path internally (wrapping the empty
# args in an Array), and `&block` must forward.
class Echo
  def echo(&block); "Echo[#{block.call}]"; end
end
class LoudEcho < Echo
  def echo(&block)
    super(&block)
  end
end
puts LoudEcho.new.echo { "FORWARDED" }
