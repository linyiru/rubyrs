# Default-inspect / %p / pattern-failure message family
# (minitest assertions' hex-diff + assert_match + assert_pattern):
# - Object#inspect carries the ivar tail (single ivar — our tail is
#   name-sorted, a documented divergence from CRuby's insertion
#   order, so multi-ivar order isn't pinned here).
# - sprintf %p dispatches a user/singleton inspect, including on
#   Hash/Array ELEMENTS (via the pre-render override channel).
# - A String carrying a singleton `==` override is honored by
#   operator syntax (BinOp gate).
# - Regexp#=~ coerces a non-String operand through to_str.
# - NoMatchingPatternError carries CRuby's "length mismatch" tail
#   for fixed-size array patterns.

class IvOne
  def initialize
    @name = "a"
  end
end
puts IvOne.new.inspect.sub(/0x[0-9a-f]+/, "0xX")
puts "%p" % [IvOne.new] =~ /@name="a"/ ? "ivar-in-p" : "missing"

obj = Object.new
obj.define_singleton_method(:inspect) { "#<CUSTOM>" }
p({ 1 => obj })
puts "%p" % [{ 1 => obj }]
puts "%p" % [[obj]]
puts "%s %p" % ["x", obj]

exp = "blah" * 3
act = "blah" * 3
def exp.==(_other)
  false
end
p(exp == act)
p(act == exp)
p(exp == "nope")

matchee = Object.new
def matchee.to_str
  "blah"
end
p(/blah/ =~ matchee)
p(/zzz/ =~ matchee)

begin
  eval "[1, 2, 3] => [Integer, Integer]"
rescue NoMatchingPatternError => e
  puts e.message
end
begin
  case [1, 2]
  in [Integer, Integer, Integer]
    :no
  end
rescue NoMatchingPatternError => e
  puts e.message
end
begin
  5 => [Integer]
rescue NoMatchingPatternError => e
  puts e.message
end
# Rest-splat / non-array-failure messages keep the bare inspect
# form (a documented narrowing — CRuby has per-failure-kind
# wording the desugar can't reconstruct).
begin
  [1] => [String, *]
rescue NoMatchingPatternError => e
  puts e.class
end
