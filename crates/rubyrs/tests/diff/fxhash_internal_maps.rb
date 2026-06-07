# Exercises the internal tables the FxHash swap touched — instance
# ivars, class-method lookup, class vars, constants, and Hash-subclass
# ivars (via attr_reader) — for content correctness across the hasher
# change. (Avoids instance_methods/instance_variables surfaces that have
# unrelated pre-existing divergences.)
class Widget
  CONST_A = 1
  CONST_B = 2
  @@count = 0
  def initialize(n)
    @a = n
    @b = n * 2
    @c = n * 3
    @@count += 1
  end
  def total
    @a + @b + @c
  end
  attr_reader :a, :b, :c
  def self.count
    @@count
  end
end

ws = (1..50).map { |i| Widget.new(i) }
p ws.map(&:total).sum
p ws.first.instance_variables.sort
p Widget.count
p [ws[0].respond_to?(:total), ws[0].respond_to?(:a), ws[0].respond_to?(:nope)]
p Widget.constants.sort
p [Widget::CONST_A, Widget::CONST_B]
p ws[25].a + ws[25].b + ws[25].c

# Hash subclass with an instance variable read back via attr_reader
# (HashObj.ivars, also FxHash now).
class TaggedHash < Hash
  def initialize(tag)
    super()
    @tag = tag
  end
  attr_reader :tag
end
th = TaggedHash.new("x")
th[:k1] = 1
th[:k2] = 2
p [th.tag, th[:k1], th[:k2], th.size]
