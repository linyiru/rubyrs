# Bare `super` (no parens) from a method with KEYWORD params must
# forward them as keywords, not positionals. Pre-fix, the kw slots were
# dumped as positional args, so a kwarg-only parent reported "wrong
# number of arguments (given N, expected 0)". Surfaced by public_suffix's
# `Wildcard#initialize(value:, length:, private:); super; end`.

class Base
  def initialize(value:, length: nil, private: false)
    @value = value
    @length = length || (@value.count(".") + 1)
    @private = private
  end
  def show; [@value, @length, @private]; end
end
class Wildcard < Base
  def initialize(value:, length: nil, private: false)
    super
    @length = (length || @length) + 1
  end
end
p Wildcard.new(value: "co.uk").show      # ["co.uk", 3, false]
p Wildcard.new(value: "x", private: true).show  # ["x", 2, true]

# Sub's own default wins, then forwards (current slot value).
class B; def m(a: 1, b: 2); [a, b]; end; end
class S < B; def m(a: 9, b: 8); super; end; end
p S.new.m                # [9, 8]
p S.new.m(a: 5)          # [5, 8]

# Positional params alongside keywords.
class P; def initialize(x, y:, z: 0); @v = [x, y, z]; end; def v; @v; end; end
class Q < P; def initialize(x, y:, z: 0); super; end; end
p Q.new(1, y: 2).v       # [1, 2, 0]
p Q.new(1, y: 2, z: 3).v # [1, 2, 3]

# Block forwarded alongside keywords.
class BlkBase; def m(a:); yield a; end; end
class BlkSub < BlkBase; def m(a:); super; end; end
p BlkSub.new.m(a: 10) { |v| v * 2 }   # 20
