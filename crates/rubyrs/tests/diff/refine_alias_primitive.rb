# `refine Array do alias :orig_sum :sum end`: aliasing a builtin inside a
# refinement resolves the primitive from the refined class (ActiveSupport's
# Array#sum override shape), and the forwarder hits the primitive directly
# so a later `def sum` calling `orig_sum` does NOT loop.
using Module.new {
  refine Array do
    alias :orig_sum :sum
  end
}
class Array
  def sum(init = nil)
    init ||= 0
    orig_sum(init)
  end
end
p [1, 2, 3].sum
p [1, 2, 3].sum(10)
p [10, 20].sum
