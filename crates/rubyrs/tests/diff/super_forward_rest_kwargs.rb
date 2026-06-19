# Bare `super` from a method that has BOTH a `*rest` splat and
# keyword params / `**kwrest` must forward the rest SPLATTED (not as
# a single nested-array positional) and the keywords as keywords.
# Surfaced by mustermann's `Concat#initialize(*, **); super; end`,
# which collapsed `[Identity, Sinatra]` into a single nested Composite.

class Base
  def initialize(patterns, operator: :|, **opts)
    p [:base, patterns, operator, opts]
  end
end

# anonymous splat + anonymous double-splat
class AnonBoth < Base
  def initialize(*, **)
    super
  end
end
AnonBoth.new([1, 2], type: :x)

# named splat + named double-splat
class NamedBoth < Base
  def initialize(*args, **kw)
    super
  end
end
NamedBoth.new([3, 4], type: :y)

# empty kwargs must forward as NO kwargs (not an extra positional)
class TwoPos
  def initialize(a, b)
    p [:twopos, a, b]
  end
end
class FwdEmpty < TwoPos
  def initialize(*a, **kw)
    super
  end
end
FwdEmpty.new(1, 2)

# pre-rest positional + rest + required keyword
class KwReq
  def initialize(x, *rest, foo:)
    p [:kwreq, x, rest, foo]
  end
end
class FwdKwReq < KwReq
  def initialize(x, *rest, foo:)
    super
  end
end
FwdKwReq.new(1, 2, 3, foo: 9)

# rest + named kw + kwrest (named keyword wins over kwrest)
class Mixed
  def initialize(*a, foo:, **rest)
    p [:mixed, a, foo, rest]
  end
end
class FwdMixed < Mixed
  def initialize(*a, foo:, **rest)
    super
  end
end
FwdMixed.new(1, foo: 2, bar: 3, baz: 4)

# rest + kwrest + a forwarded block
class WithBlock
  def initialize(*a, **kw)
    p [:withblock, a, kw, block_given?]
  end
end
class FwdBlock < WithBlock
  def initialize(*a, **kw, &blk)
    super
  end
end
FwdBlock.new(1, 2, x: 3) { }
