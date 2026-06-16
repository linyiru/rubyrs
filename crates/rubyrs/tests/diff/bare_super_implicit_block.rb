# Bare `super` forwards the calling frame's block to the superclass
# method even when the overriding method never names it with a `&blk`
# param. This is the mustermann `Capture#parse` shape: an override does
# `super` with no block param, and the parent body `yield`s.
class Base
  def parse
    self.collected ||= []
    while (el = yield)
      self.collected << el
    end
    self.collected
  end
  attr_accessor :collected
end

class Capture < Base
  def parse            # NO &blk param — block flows implicitly
    self.collected ||= "seed:"
    super
  end
end

# direct literal block
src = [1, 2, 3, nil]
i = -1
p Capture.new.parse { i += 1; src[i] }

# block forwarded via &block at the call site (n.parse(&block))
gen = proc { j = (@j ||= -1) + 1; @j = j; %w[a b nil][j] && (%w[a b][j]) }
obj = Capture.new
p obj.parse(&gen)

# no block at all -> block_given? false in the parent's yield site
class Quiet < Base
  def parse
    block_given? ? super : "no-block-given"
  end
end
p Quiet.new.parse
