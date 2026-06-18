# Bare `new(...) { block }` (implicit receiver = self class) must forward
# the block to #initialize, including via bare `super(...) { block }` —
# Sinatra/mustermann's `def self.new(s, **o); super(s, **o) { o }; end`.
class Base
  def initialize(s, **opts)
    @s = s
    @options = yield.freeze if block_given?
  end
  attr_reader :s, :options
end
class Child < Base
  def self.b1(x);      new(x)       { "blk" }; end
  def self.b2(x, **o); new(x, **o)  { o };     end
  def self.via_super(string, **options)
    new(string, **options)
  end
  def self.new(string, **options)
    super(string, **options) { options }
  end
end
p Child.b1(1).options
p Child.b2(2, a: 1).options
c = Child.new("/p", x: 9)
p [c.s, c.options]
# explicit-receiver form must stay identical
class Plain
  def initialize(*); @b = block_given? ? yield : "none"; end
  attr_reader :b
end
p Plain.new(1) { "yes" }.b
p Plain.new(1).b
