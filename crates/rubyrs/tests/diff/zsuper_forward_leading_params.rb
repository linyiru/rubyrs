# Bare `super` inside `def m(a, ...)` forwards the leading positional
# params (their CURRENT values) ahead of the `...` args.
class FwdParent
  def m(a, *rest, **kw, &blk)
    [a, rest, kw, blk&.call]
  end
end

class FwdOnly < FwdParent
  def m(...) = super
end

class FwdLead < FwdParent
  def m(a, ...) = super
end

class FwdTwoLead < FwdParent
  def m(a, b, ...) = super
end

class FwdReassign < FwdParent
  def m(a, ...)
    a = a * 10
    super
  end
end

p FwdOnly.new.m(1, 2, k: 3) { 4 }
p FwdLead.new.m(1)
p FwdLead.new.m(1, 2, 3, k: 4) { 5 }
p FwdTwoLead.new.m(1, 2, 3)
p FwdReassign.new.m(1, 2)
