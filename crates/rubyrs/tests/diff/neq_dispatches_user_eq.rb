# `BasicObject#!=` is `!(self == other)` with `==` dispatched, so a
# user-defined `==` decides `!=` too. ActiveRecord's `insert_all` compares
# key Sets with `!=` (the Set half is `neq_set.rb`, stdlib-gated).

class Pt
  attr_reader :x
  def initialize(x) = @x = x
  def ==(o) = o.is_a?(Pt) && x == o.x
end
p Pt.new(1) != Pt.new(1)   # false
p Pt.new(1) != Pt.new(2)   # true
p Pt.new(1) != 1           # true
p Pt.new(1).send(:!=, Pt.new(1))
p Pt.new(1).public_send(:!=, Pt.new(2))
p Pt.new(1).method(:!=).call(Pt.new(1))
p [Pt.new(1), Pt.new(2)].map { |q| q != Pt.new(1) }

# Truthiness of the `==` result, not identity with true.
class Loose
  def ==(o) = o == :yes ? 1 : nil
end
p Loose.new != :yes, Loose.new != :no

# An explicit `!=` still wins over the derived one.
class Contrary
  def ==(_) = true
  def !=(_) = :own
end
p Contrary.new != Contrary.new

# Inherited `==` and `==` from a module.
class SubPt < Pt; end
p SubPt.new(3) != SubPt.new(3)
module Always
  def ==(_) = true
end
class Mixed; include Always; end
p Mixed.new != Object.new

# Plain objects keep identity semantics.
o = Object.new
p o != o, o != Object.new

# Class-level `==`.
class Tag
  def self.==(o) = o == :tag
end
p Tag != :tag, Tag != :other

# Struct, whose `==` compares members.
S = Struct.new(:a)
p S.new(1) != S.new(1), S.new(1) != S.new(2)

# `==` raising propagates out of `!=`.
class Boom
  def ==(_) = raise("boom")
end
begin
  Boom.new != 1
rescue => e
  p e.message
end
