# Bare `super` from a method with named kwargs + **kwrest forwards the
# named params merged over the kwrest hash as KEYWORDS (not a positional
# hash). mustermann's Composite.supported?(option, type: nil, **options).
class Base
  def self.sup(option, **options); "opt=#{option} options=#{options}"; end
  def initialize(value:, **rest); @s = "v=#{value} rest=#{rest}"; end
  attr_reader :s
end
class Child < Base
  def self.sup(option, type: nil, **options); super; end
end
p Child.sup(:x)
p Child.sup(:x, type: :foo, extra: 1)

# initialize variant (kwargs + kwrest, no positional)
class C2 < Base
  def initialize(value:, length: 0, **rest); super; end
end
p C2.new(value: 1, length: 2, private: true).s
