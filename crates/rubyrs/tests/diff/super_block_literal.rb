# `super do |...| ... end` forwards a block LITERAL to the parent
# method (distinct from `super(&proc)`). Covers bare-arg forwarding,
# explicit args, and the parent yielding the block.
class Base
  def go(x); yield x; "base:#{x}"; end
  def fwd(a, b); yield(a + b); "fwd"; end
end
class Child < Base
  def go(x); super { |v| puts "blk #{v}" }; end
  def fwd(a, b); super(a, b) { |s| puts "sum #{s}" }; end
end
p Child.new.go(7)
p Child.new.fwd(2, 3)
