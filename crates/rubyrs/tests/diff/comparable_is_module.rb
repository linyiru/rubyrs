# Comparable is a MODULE (not a Class): its #class is Module, it is not
# is_a?(Class), and #instance_methods lists only its own 7 methods
# (no universal-Object surface). The mixin machinery is unaffected.
p Comparable.class
p Comparable.is_a?(Module)
p Comparable.is_a?(Class)
p Comparable.instance_methods.sort
p Comparable.instance_methods(false).sort

# Mixin still works everywhere.
p [3, 1, 2].sort
p (1 < 2)
p 5.between?(1, 10)
p 5.clamp(1, 3)
p "b" < "c"
p Integer.include?(Comparable)
p 5.is_a?(Comparable)

class Box
  include Comparable
  attr_reader :v
  def initialize(v); @v = v; end
  def <=>(o); v <=> o.v; end
end
p [Box.new(3), Box.new(1), Box.new(2)].sort.map(&:v)
p Box.new(2) > Box.new(1)
p Box.new(5).clamp(Box.new(1), Box.new(3)).v
p Box.ancestors.include?(Comparable)
