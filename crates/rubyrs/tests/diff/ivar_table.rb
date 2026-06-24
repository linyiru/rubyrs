# IvarTable: insertion-ordered linear Vec for instance ivars. Exercise
# order (now CRuby-parity, not alpha-sorted), reflection, removal, dup.
class Pt
  def initialize(x, y); @x = x; @y = y; end
  def to_s; "#{@x},#{@y}"; end
end
pt = Pt.new(3, 4)
puts pt.to_s
p2 = Pt.new(1, 2); p2.instance_variable_set(:@x, 99)
puts p2.instance_variable_get(:@x)
p pt.instance_variables
# non-alphabetical definition order must be preserved (CRuby parity)
class Rev; def initialize; @z = 1; @a = 2; @m = 3; end; end
p Rev.new.instance_variables
# many ivars: insertion order intact past any small-buffer boundary
class Many; def initialize; (1..12).each { |i| instance_variable_set(:"@v#{i}", i * 10) }; end; end
m = Many.new
p m.instance_variables.length
p m.instance_variables.first(4)
p m.instance_variable_get(:@v1)
p m.instance_variable_get(:@v12)
p m.instance_variable_defined?(:@v7)
p m.instance_variable_defined?(:@nope)
m.remove_instance_variable(:@v1)
p m.instance_variable_defined?(:@v1)
p m.instance_variables.length
d = pt.dup; d.instance_variable_set(:@x, -1)
p [pt.instance_variable_get(:@x), d.instance_variable_get(:@x)]
p2.instance_variable_set(:@y, 7); p p2.instance_variables
