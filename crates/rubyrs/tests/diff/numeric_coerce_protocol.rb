# Numeric coerce protocol: a built-in numeric LHS sent a coercible
# arithmetic operator with a non-numeric RHS runs `a, b =
# rhs.coerce(lhs); a.send(op, b)`. A non-coercible RHS raises the
# canonical "X can't be coerced into <Numeric>" TypeError (not a
# misleading NoMethodError).

class Scaled
  attr_reader :v
  def initialize(v); @v = v; end
  def coerce(other); [Scaled.new(other), self]; end
  def +(o); Scaled.new(@v + o.v); end
  def *(o); Scaled.new(@v * o.v); end
  def to_s; "Scaled(#{@v})"; end
  def inspect; to_s; end
end

p (10 + Scaled.new(5))
p (3 * Scaled.new(4))
p (2.0 + Scaled.new(1))

# non-coercible RHS → canonical TypeError per operator
[
  -> { 1 + "x" },
  -> { 1 - nil },
  -> { 1 * :sym },
  -> { 1 / [] },
  -> { 1.0 + "x" },
  -> { 2 ** "x" },
].each do |l|
  begin
    p l.call
  rescue => e
    p [e.class, e.message]
  end
end

# comparison ops keep their existing (correct) behaviour
p (1 == "x")
p (1 <=> "x")
begin
  1 < "x"
rescue => e
  p [e.class, e.message]
end
