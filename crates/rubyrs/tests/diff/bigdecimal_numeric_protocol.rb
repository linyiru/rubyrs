# BigDecimal inherits CRuby's Numeric ancestry and its real-number
# complex-decomposition protocol (real / imaginary / conjugate /
# rectangular / polar / arg / abs2 / magnitude / real? / integer?),
# plus the Rational-derived numerator / denominator / fdiv. BigDecimal
# values are compared via to_f / to_i (and BigDecimal results unwrapped
# with to_f) to stay clear of the BigDecimal display divergence (rubyrs
# prints "12.25", CRuby "0.1225e2").
require "bigdecimal"

d = BigDecimal("3.5")

p BigDecimal.ancestors.include?(Numeric)   # true
p d.is_a?(Numeric)                          # true

# Numeric protocol (BigDecimal-valued results unwrapped via to_f).
p d.real.to_f                               # 3.5
p d.imaginary                               # 0
p d.imag                                    # 0
p d.conjugate.to_f                          # 3.5
p d.conj.to_f                               # 3.5
p d.real?                                   # true
p d.rectangular.map { |x| x.respond_to?(:to_f) ? x.to_f : x }  # [3.5, 0]
p d.rect.map { |x| x.respond_to?(:to_f) ? x.to_f : x }         # [3.5, 0]
p d.polar.map { |x| x.respond_to?(:to_f) ? x.to_f : x }        # [3.5, 0]
p d.arg                                      # 0
p (-d).arg                                   # PI
p d.abs2.to_f                                # 12.25
p d.magnitude.to_f                           # 3.5
p d.integer?                                 # false

# Rational-derived helpers.
p d.numerator                                # 7
p d.denominator                              # 2
p d.fdiv(2).to_f                             # 1.75

# Duck typing through case/when Numeric.
case d
when Integer then puts "int"
when Numeric then puts "numeric"
else puts "other"
end
