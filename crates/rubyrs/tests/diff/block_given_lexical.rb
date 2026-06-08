# block_given? resolves the LEXICAL enclosing method's block (same as
# yield), not the call-stack-nearest method — they diverge when the block
# runs through a user iterator that itself has a block.
def helper; yield; end
def outer; helper { block_given? }; end
p outer                            # false (outer has no block)
p outer { }                        # true

# the Enumerable shape that exposed it
class C
  include Enumerable
  def initialize(*a); @a = a; end
  def each; @a.each { |x| yield x }; end
end
# min with no block must NOT take the block branch
p C.new(3, 1, 2).min               # 1
p C.new(3, 1, 2).max               # 3
# min WITH a comparator block
p C.new(3, 1, 2).min { |a, b| b <=> a }  # 3
p C.new(1, 2, 3).count             # 3 (no block → count all)
p C.new(1, 2, 3).count { |x| x > 1 }  # 2

# nested: block_given? deep inside, through two user iterators
def twice; yield; yield; end
def m; r = []; twice { r << block_given? }; r; end
p m                                # [false, false]
p m { }                            # [true, true]
