# `super` from a block-form Struct subclass's custom initialize must
# reach the generated member-assigner. Driver: parser's
# Source::Map::Variable. Also covers initialize_copy super (dup hook).
S = Struct.new(:a, :b, :c) do
  def initialize(a, b, c)
    super
    @extra = a + b
  end
  def extra; @extra; end
end
s = S.new(10, 20, 30)
p [s.a, s.b, s.c, s.extra]

T = Struct.new(:x, :y) do
  def initialize(x, y); super(x, y); end
end
p [T.new(1, 2).x, T.new(1, 2).y]

# initialize_copy super via dup
class Holder
  attr_accessor :data
  def initialize_copy(other); super; @dup_ran = true; end
  def dup_ran; @dup_ran; end
end
h = Holder.new; h.data = [1, 2]
d = h.dup
p [d.data, d.dup_ran, d.equal?(h)]
