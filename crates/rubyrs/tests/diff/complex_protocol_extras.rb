# Complex methods beyond the arithmetic core: numerator / denominator
# (clear the fractional parts over a common denominator), finite? /
# infinite? (fold over both components), zero? / nonzero?, and
# rationalize (real-valued only). Byte-stable against CRuby.

p Complex(3, 4).numerator        # (3+4i)
p Complex(3, 4).denominator      # 1
p Complex(Rational(1, 2), Rational(1, 3)).denominator  # 6
p Complex(Rational(1, 2), Rational(1, 3)).numerator    # (3+2i)

p Complex(3, 4).finite?          # true
p Complex(3, 4).infinite?        # nil
p Complex(Float::INFINITY, 0).finite?    # false
p Complex(Float::INFINITY, 0).infinite?  # 1
p Complex(0, Float::INFINITY).infinite?  # 1

p Complex(0, 0).zero?            # true
p Complex(3, 4).zero?            # false
p Complex(0, 0).nonzero?         # nil
p Complex(3, 4).nonzero?         # (3+4i)

p Complex(3, 0).rationalize      # (3/1)
p Complex(Rational(1, 2), 0).rationalize  # (1/2)
begin
  Complex(3, 4).rationalize
rescue => e
  puts "#{e.class}: #{e.message}"  # RangeError
end
