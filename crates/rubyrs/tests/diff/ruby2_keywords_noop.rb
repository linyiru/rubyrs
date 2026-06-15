# `ruby2_keywords(:m)` marks a `*args` method to preserve a trailing
# keyword hash through delegation. rubyrs already collects trailing
# kwargs into the rest param as a Hash, so the flag is a no-op (returns
# nil, like CRuby). Surfaced by faraday's RackBuilder::Handler.
class Delegator
  def call(*args)
    args
  end
  ruby2_keywords :call
end

p Delegator.new.call(1, 2, k: 3)

# Module body form + the return value (nil).
module M
  def opts(*a); a; end
  RESULT = ruby2_keywords(:opts)
end
p M::RESULT

class C2
  include M
end
p C2.new.opts(:x, y: 1)
