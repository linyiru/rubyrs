# Splat + explicit `&block` co-existing in a single call, plus
# bareword `instance_eval(&block)`. Surfaced by sinatra_lite's
# Rack middleware chain (`klass.new(inner_app, *args, &block)`)
# and rack-cors's `instance_eval(&block)` DSL initialize.

class Forwarder
  def initialize(*pos, &blk)
    @pos = pos
    @block_given = block_given?
    @block_result = blk.call if blk
  end

  attr_reader :pos, :block_given, :block_result
end

# Splat without block — baseline.
f1 = Forwarder.new(*[1, 2, 3])
puts "f1.pos=#{f1.pos.inspect} block_given=#{f1.block_given}"

# Block without splat — baseline.
f2 = Forwarder.new("hi") { 42 }
puts "f2.pos=#{f2.pos.inspect} block_given=#{f2.block_given} result=#{f2.block_result}"

# Splat with empty args + block — the rack-cors regression shape.
args = []
blk = lambda { "from-lambda" }
f3 = Forwarder.new(*args, &blk)
puts "f3.pos=#{f3.pos.inspect} block_given=#{f3.block_given} result=#{f3.block_result}"

# Splat with one arg + block.
args2 = ["one"]
f4 = Forwarder.new(*args2, &blk)
puts "f4.pos=#{f4.pos.inspect} block_given=#{f4.block_given} result=#{f4.block_result}"

# Multiple splatted args + block.
f5 = Forwarder.new(*[10, 20, 30], &blk)
puts "f5.pos=#{f5.pos.inspect} block_given=#{f5.block_given} result=#{f5.block_result}"

# Bareword `instance_eval(&block)` inside a method — uses self
# (which is the receiver of the enclosing method) without explicit
# receiver. Rack::Cors's initialize uses this shape to apply the
# configuration block.
class DslHost
  attr_reader :tag, :value
  def initialize(&block)
    @tag = "init"
    instance_eval(&block) if block_given?
  end
  def set_value(v); @value = v; end
end

host = DslHost.new do
  set_value(123)
end
puts "host.tag=#{host.tag} value=#{host.value}"
