# `defined?(yield)` — "yield" when the enclosing method was called with
# a block, else nil. sequel's Database.connect gates `return yield(db)`
# on `if defined?(yield)`; with no block it must skip the yield.
def gated
  if defined?(yield)
    yield(10)
  else
    :no_block
  end
end
p gated { |x| x * 2 }          # 20
p gated                        # :no_block

def label; defined?(yield); end
p label { }                    # "yield"
p label                        # nil

# defined?(yield) resolves the ENCLOSING method's block through an
# iterator block (same lexical-owner rule as block_given?)
def outer
  [1].each { return (defined?(yield) ? yield : :none) }
end
p outer { :from_outer }        # :from_outer
p outer                        # :none

# ensure-block guard pattern (sequel's shape)
def with_resource
  r = :resource
  begin
    return yield(r) if defined?(yield)
  ensure
    @cleaned = true if defined?(yield)
  end
  r
end
p with_resource { |x| [x, :used] }   # [:resource, :used]
p with_resource                       # :resource
