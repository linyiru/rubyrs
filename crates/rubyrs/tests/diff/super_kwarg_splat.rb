# `super(arg, **kwargs)` and `super(arg, k: v)` — kwarg sugar
# in a super call's argument list. Pre-fix, the super-args walk
# called `tr()` directly on each arg node, which tripped the
# `unsupported node: KeywordHashNode` trap when an arg list
# contained `**opts` or trailing kwargs. Mirrors the regular
# Call-args walk's `as_keyword_hash_node()` routing through
# `tr_kwhash`.
#
# This is the P3 Sinatra-spike gap that blocked
# `require 'mustermann/pattern'` at line 59:
#   @map.fetch([string, options]) { super(string, **options) { options } }

# 1. `super(arg, **opts)` — kwargs splat after a positional arg.
# Parent has `**opts` to absorb the splat back into a Hash.
class P1
  def self.new(s, **opts)
    "p1: s=#{s} opts=#{opts.inspect}"
  end
end
class C1 < P1
  def self.new(s, **options)
    super(s, **options)
  end
end
puts C1.new("hello", a: 1, b: 2)
puts C1.new("solo")  # empty kwargs splat

# 2. `super(*input, **opts)` — splat AND kwargs splat in the
# same super call. Mustermann's
# `self[:regexp].new(input, **options)` shape generalised to
# super.
class P2
  def self.new(*input, **opts)
    "p2: input=#{input.inspect} opts=#{opts.inspect}"
  end
end
class C2 < P2
  def self.new(*input, **options)
    super(*input, **options)
  end
end
puts C2.new("a", "b", x: 1)
puts C2.new("only")

# 3. Trailing literal kwargs (no splat): `super(arg, k: v)`.
class P3
  def self.new(s, **opts)
    "p3: s=#{s} opts=#{opts.inspect}"
  end
end
class C3 < P3
  def self.new(s)
    super(s, key: "value", count: 42)
  end
end
puts C3.new("x")

# 4. Mixed: positional, splat, AND trailing kwargs splat.
class P4
  def self.new(*pos, **opts)
    "p4: pos=#{pos.inspect} opts=#{opts.inspect}"
  end
end
class C4 < P4
  def self.new(*rest, **options)
    super("prefix", *rest, **options)
  end
end
puts C4.new("a", "b", x: 1, y: 2)
puts C4.new
