# IvarTable: linear Vec for small instances (insertion-ordered, no
# hashing) spilling to a HashMap past 8 ivars. Exercise order, the
# spill boundary, reflection, removal, and dup — all must match CRuby.
class Pt
  def initialize(x, y); @x = x; @y = y; end
  def to_s; "#{@x},#{@y}"; end
end
pt = Pt.new(3, 4)
puts pt.to_s
p2 = Pt.new(1, 2); p2.instance_variable_set(:@x, 99)
puts p2.instance_variable_get(:@x)
p pt.instance_variables                      # [:@x, :@y] in definition order
# many ivars -> crosses the spill threshold (8); order + lookup intact
class Many
  def initialize
    (1..12).each { |i| instance_variable_set(:"@v#{i}", i * 10) }
  end
end
m = Many.new
p m.instance_variables.length               # 12
p m.instance_variable_get(:@v1)
p m.instance_variable_get(:@v12)
p m.instance_variable_defined?(:@v7)
p m.instance_variable_defined?(:@nope)
m.remove_instance_variable(:@v1)
p m.instance_variable_defined?(:@v1)
p m.instance_variables.length               # 11
# dup copies the ivar table independently
d = pt.dup; d.instance_variable_set(:@x, -1)
p [pt.instance_variable_get(:@x), d.instance_variable_get(:@x)]
# reopen-after-read: overwrite existing keeps order/position
p2.instance_variable_set(:@y, 7); p p2.instance_variables
