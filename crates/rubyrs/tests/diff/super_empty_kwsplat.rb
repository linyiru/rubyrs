# `super(**k)` / `super(*a, **k)` with an EMPTY kwsplat must forward NO keywords
# (CRuby keyword separation) — not an empty Hash positional. A non-empty kwsplat
# forwards as keywords; positionals stay positional. (minitest's stub forwards
# `Class#new` via `def new(*a, **k) super(*a, **k) end`, hit by zeitwerk.)
class Base0
  def m; "0 args"; end
end
class KwOnly < Base0; def m(**k); super(**k); end; end
class SplatKw < Base0; def m(*a, **k); super(*a, **k); end; end
puts KwOnly.new.m            # "0 args" — empty **k dropped
puts SplatKw.new.m           # "0 args" — both empty

class BaseKw
  def m(x:, y:); "x=#{x} y=#{y}"; end
end
class FwdKw < BaseKw; def m(*a, **k); super(*a, **k); end; end
puts FwdKw.new.m(x: 1, y: 2) # "x=1 y=2" — kwargs forwarded

class BasePos
  def m(a, b); "a=#{a} b=#{b}"; end
end
class FwdPos < BasePos; def m(*a, **k); super(*a, **k); end; end
puts FwdPos.new.m(10, 20)    # "a=10 b=20" — positionals stay positional
